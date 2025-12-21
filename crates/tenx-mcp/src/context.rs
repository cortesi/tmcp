use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tokio::sync::broadcast;

use crate::{
    api::{ClientAPI, ServerAPI},
    error::{Error, Result},
    request_handler::{RequestHandler, TransportSink},
    schema::{self},
};

/// Context provided to ClientConnection implementations for interacting with the server
///
/// This context is only valid for the duration of a single method call and should not
/// be stored or used outside of that scope. The Clone implementation is for internal
/// framework use only.
#[derive(Clone)]
pub struct ClientCtx {
    /// Sender for client notifications
    pub(crate) notification_tx: broadcast::Sender<schema::ClientNotification>,
    /// Request handler for making requests to server
    request_handler: RequestHandler,
    /// The current request ID, if this context is handling a request
    pub(crate) request_id: Option<schema::RequestId>,
}

impl ClientCtx {
    /// Create a new ClientConnectionContext with the given notification sender
    pub(crate) fn new(
        notification_tx: broadcast::Sender<schema::ClientNotification>,
        transport_tx: Option<TransportSink>,
    ) -> Self {
        Self {
            notification_tx,
            request_handler: RequestHandler::new(transport_tx, "client-req".to_string()),
            request_id: None,
        }
    }

    /// Send a notification to the client
    pub fn send_notification(&self, notification: schema::ClientNotification) -> Result<()> {
        self.notification_tx
            .send(notification)
            .map_err(|_| Error::InternalError("Failed to send notification".into()))?;
        Ok(())
    }

    /// Create a new context with a specific request ID
    pub(crate) fn with_request_id(&self, request_id: schema::RequestId) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx
    }

    /// Send a request to the server and wait for response
    async fn request<T>(&self, request: schema::ClientRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.request_handler.request(request).await
    }

    /// Send a cancellation notification for the current request
    pub fn cancel(&self, reason: Option<String>) -> Result<()> {
        if let Some(request_id) = &self.request_id {
            self.send_notification(schema::ClientNotification::Cancelled {
                request_id: request_id.clone(),
                reason,
            })
        } else {
            Err(Error::InternalError(
                "No request ID available to cancel".into(),
            ))
        }
    }
}

/// Implementation of ServerAPI trait for ClientCtx
#[async_trait]
impl ServerAPI for ClientCtx {
    async fn initialize(
        &mut self,
        protocol_version: String,
        capabilities: schema::ClientCapabilities,
        client_info: schema::Implementation,
    ) -> Result<schema::InitializeResult> {
        self.request(schema::ClientRequest::Initialize {
            protocol_version,
            capabilities,
            client_info,
        })
        .await
    }

    async fn ping(&mut self) -> Result<()> {
        let _: schema::EmptyResult = self.request(schema::ClientRequest::Ping).await?;
        Ok(())
    }

    async fn list_tools(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListToolsResult> {
        self.request(schema::ClientRequest::ListTools {
            cursor: cursor.into(),
        })
        .await
    }

    async fn call_tool(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<schema::CallToolResult> {
        self.request(schema::ClientRequest::CallTool {
            name: name.into(),
            arguments,
        })
        .await
    }

    async fn list_resources(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListResourcesResult> {
        self.request(schema::ClientRequest::ListResources {
            cursor: cursor.into(),
        })
        .await
    }

    async fn list_resource_templates(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListResourceTemplatesResult> {
        self.request(schema::ClientRequest::ListResourceTemplates {
            cursor: cursor.into(),
        })
        .await
    }

    async fn resources_read(
        &mut self,
        uri: impl Into<String> + Send,
    ) -> Result<schema::ReadResourceResult> {
        self.request(schema::ClientRequest::ReadResource { uri: uri.into() })
            .await
    }

    async fn resources_subscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: schema::EmptyResult = self
            .request(schema::ClientRequest::Subscribe { uri: uri.into() })
            .await?;
        Ok(())
    }

    async fn resources_unsubscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: schema::EmptyResult = self
            .request(schema::ClientRequest::Unsubscribe { uri: uri.into() })
            .await?;
        Ok(())
    }

    async fn list_prompts(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListPromptsResult> {
        self.request(schema::ClientRequest::ListPrompts {
            cursor: cursor.into(),
        })
        .await
    }

    async fn get_prompt(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<schema::GetPromptResult> {
        self.request(schema::ClientRequest::GetPrompt {
            name: name.into(),
            arguments,
        })
        .await
    }

    async fn complete(
        &mut self,
        reference: schema::Reference,
        argument: schema::ArgumentInfo,
    ) -> Result<schema::CompleteResult> {
        self.request(schema::ClientRequest::Complete {
            reference,
            argument,
            context: None,
        })
        .await
    }

    async fn set_level(&mut self, level: schema::LoggingLevel) -> Result<()> {
        let _: schema::EmptyResult = self
            .request(schema::ClientRequest::SetLevel { level })
            .await?;
        Ok(())
    }
}

/// Context provided to ServerConn implementations for interacting with clients
///
/// This context is only valid for the duration of a single method call and should not
/// be stored or used outside of that scope. The Clone implementation is for internal
/// framework use only.
#[derive(Clone)]
pub struct ServerCtx {
    /// Sender for server notifications
    pub(crate) notification_tx: broadcast::Sender<schema::ServerNotification>,
    /// Request handler for making requests to clients
    request_handler: RequestHandler,
    /// The current request ID, if this context is handling a request
    pub(crate) request_id: Option<schema::RequestId>,
}

impl ServerCtx {
    /// Create a new ServerCtx with notification channel and transport
    pub(crate) fn new(
        notification_tx: broadcast::Sender<schema::ServerNotification>,
        transport_tx: Option<TransportSink>,
    ) -> Self {
        Self {
            notification_tx,
            request_handler: RequestHandler::new(transport_tx, "srv-req".to_string()),
            request_id: None,
        }
    }

    /// Send a notification to the client
    pub fn notify(&self, notification: schema::ServerNotification) -> Result<()> {
        self.notification_tx
            .send(notification)
            .map_err(|_| Error::InternalError("Failed to send notification".into()))?;
        Ok(())
    }

    /// Create a new context with a specific request ID
    pub(crate) fn with_request_id(&self, request_id: schema::RequestId) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx
    }

    /// Send a request to the client and wait for response
    async fn request<T>(&self, request: schema::ServerRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.request_handler.request(request).await
    }

    /// Handle a response from the client
    pub(crate) async fn handle_client_response(&self, response: schema::JSONRPCResponse) {
        // Clone the handler to avoid holding locks across await points
        let handler = self.request_handler.clone();
        handler.handle_response(response).await
    }

    /// Handle an error response from the client
    pub(crate) async fn handle_client_error(&self, error: schema::JSONRPCError) {
        // Clone the handler to avoid holding locks across await points
        let handler = self.request_handler.clone();
        handler.handle_error(error).await
    }

    /// Send a cancellation notification for the current request
    pub fn cancel(&self, reason: Option<String>) -> Result<()> {
        if let Some(request_id) = &self.request_id {
            self.notify(schema::ServerNotification::Cancelled {
                request_id: request_id.clone(),
                reason,
            })
        } else {
            Err(Error::InternalError(
                "No request ID available to cancel".into(),
            ))
        }
    }
}

/// Implementation of ClientAPI trait for ServerCtx
#[async_trait]
impl ClientAPI for ServerCtx {
    async fn ping(&mut self) -> Result<()> {
        let _: schema::EmptyResult = self.request(schema::ServerRequest::Ping).await?;
        Ok(())
    }

    async fn create_message(
        &mut self,
        params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        self.request(schema::ServerRequest::CreateMessage(Box::new(params)))
            .await
    }

    async fn list_roots(&mut self) -> Result<schema::ListRootsResult> {
        self.request(schema::ServerRequest::ListRoots).await
    }

    async fn elicit(&mut self, params: schema::ElicitParams) -> Result<schema::ElicitResult> {
        self.request(schema::ServerRequest::Elicit(params)).await
    }
}
