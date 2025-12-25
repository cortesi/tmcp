use std::collections::HashMap;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tokio::sync::broadcast;

use crate::{
    api::{ClientAPI, ServerAPI},
    error::{Error, Result},
    request_handler::{RequestHandler, TransportSink},
    schema::{self},
};

/// Context provided to `ClientHandler` implementations for interacting with the server
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
    /// Create a new `ClientCtx` with the given notification sender
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
            self.send_notification(schema::ClientNotification::cancelled(
                Some(request_id.clone()),
                reason,
            ))
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
        self.request(schema::ClientRequest::initialize(
            protocol_version,
            capabilities,
            client_info,
        ))
        .await
    }

    async fn ping(&mut self) -> Result<()> {
        let _: schema::EmptyResult = self.request(schema::ClientRequest::ping()).await?;
        Ok(())
    }

    async fn list_tools(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListToolsResult> {
        self.request(schema::ClientRequest::list_tools(cursor.into()))
            .await
    }

    async fn call_tool(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
        task: Option<schema::TaskMetadata>,
    ) -> Result<schema::CallToolResult> {
        self.request(schema::ClientRequest::call_tool(name, arguments, task))
            .await
    }

    async fn list_resources(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListResourcesResult> {
        self.request(schema::ClientRequest::list_resources(cursor.into()))
            .await
    }

    async fn list_resource_templates(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListResourceTemplatesResult> {
        self.request(schema::ClientRequest::list_resource_templates(
            cursor.into(),
        ))
        .await
    }

    async fn resources_read(
        &mut self,
        uri: impl Into<String> + Send,
    ) -> Result<schema::ReadResourceResult> {
        self.request(schema::ClientRequest::read_resource(uri))
            .await
    }

    async fn resources_subscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: schema::EmptyResult = self.request(schema::ClientRequest::subscribe(uri)).await?;
        Ok(())
    }

    async fn resources_unsubscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: schema::EmptyResult = self
            .request(schema::ClientRequest::unsubscribe(uri))
            .await?;
        Ok(())
    }

    async fn list_prompts(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListPromptsResult> {
        self.request(schema::ClientRequest::list_prompts(cursor.into()))
            .await
    }

    async fn get_prompt(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<HashMap<String, String>>,
    ) -> Result<schema::GetPromptResult> {
        self.request(schema::ClientRequest::get_prompt(name, arguments))
            .await
    }

    async fn complete(
        &mut self,
        reference: schema::Reference,
        argument: schema::ArgumentInfo,
        context: Option<schema::CompleteContext>,
    ) -> Result<schema::CompleteResult> {
        self.request(schema::ClientRequest::complete(
            reference, argument, context,
        ))
        .await
    }

    async fn set_level(&mut self, level: schema::LoggingLevel) -> Result<()> {
        let _: schema::EmptyResult = self
            .request(schema::ClientRequest::set_level(level))
            .await?;
        Ok(())
    }

    async fn get_task(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskResult> {
        self.request(schema::ClientRequest::get_task(task_id)).await
    }

    async fn get_task_payload(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskPayloadResult> {
        self.request(schema::ClientRequest::get_task_payload(task_id))
            .await
    }

    async fn list_tasks(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListTasksResult> {
        self.request(schema::ClientRequest::list_tasks(cursor.into()))
            .await
    }

    async fn cancel_task(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::CancelTaskResult> {
        self.request(schema::ClientRequest::cancel_task(task_id))
            .await
    }
}

/// Context provided to `ServerHandler` implementations for interacting with clients
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

    /// Send a cancellation notification for the current request
    pub fn cancel(&self, reason: Option<String>) -> Result<()> {
        if let Some(request_id) = &self.request_id {
            self.notify(schema::ServerNotification::cancelled(
                Some(request_id.clone()),
                reason,
            ))
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
        let _: schema::EmptyResult = self.request(schema::ServerRequest::ping()).await?;
        Ok(())
    }

    async fn create_message(
        &mut self,
        params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        self.request(schema::ServerRequest::create_message(params))
            .await
    }

    async fn list_roots(&mut self) -> Result<schema::ListRootsResult> {
        self.request(schema::ServerRequest::list_roots()).await
    }

    async fn elicit(
        &mut self,
        params: schema::ElicitRequestParams,
    ) -> Result<schema::ElicitResult> {
        self.request(schema::ServerRequest::elicit(params)).await
    }

    async fn get_task(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskResult> {
        self.request(schema::ServerRequest::get_task(task_id)).await
    }

    async fn get_task_payload(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskPayloadResult> {
        self.request(schema::ServerRequest::get_task_payload(task_id))
            .await
    }

    async fn list_tasks(
        &mut self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListTasksResult> {
        self.request(schema::ServerRequest::list_tasks(cursor.into()))
            .await
    }

    async fn cancel_task(
        &mut self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::CancelTaskResult> {
        self.request(schema::ServerRequest::cancel_task(task_id))
            .await
    }
}
