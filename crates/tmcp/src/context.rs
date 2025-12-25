use serde::de::DeserializeOwned;
use tokio::sync::broadcast;

use crate::{
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
    /// The current request ID, if this context is handling a request
    pub(crate) request_id: Option<schema::RequestId>,
}

impl ClientCtx {
    /// Create a new `ClientCtx` with the given notification sender
    pub(crate) fn new(notification_tx: broadcast::Sender<schema::ClientNotification>) -> Self {
        Self {
            notification_tx,
            request_id: None,
        }
    }

    /// Send a notification to the server
    pub fn notify(&self, notification: schema::ClientNotification) -> Result<()> {
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

    /// Send a cancellation notification for the current request
    pub fn cancel(&self, reason: Option<String>) -> Result<()> {
        if let Some(request_id) = &self.request_id {
            self.notify(schema::ClientNotification::cancelled(
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

/// MCP protocol methods for client interaction.
///
/// These methods allow a server to make requests to the connected client.
impl ServerCtx {
    /// Respond to ping requests from the client
    pub async fn ping(&self) -> Result<()> {
        let _: schema::EmptyResult = self.request(schema::ServerRequest::ping()).await?;
        Ok(())
    }

    /// Handle LLM sampling requests - ask the client to create a message
    pub async fn create_message(
        &self,
        params: schema::CreateMessageParams,
    ) -> Result<schema::CreateMessageResult> {
        self.request(schema::ServerRequest::create_message(params))
            .await
    }

    /// List available filesystem roots from the client
    pub async fn list_roots(&self) -> Result<schema::ListRootsResult> {
        self.request(schema::ServerRequest::list_roots()).await
    }

    /// Handle elicitation requests - ask the client for user input
    pub async fn elicit(
        &self,
        params: schema::ElicitRequestParams,
    ) -> Result<schema::ElicitResult> {
        self.request(schema::ServerRequest::elicit(params)).await
    }

    /// Retrieve the state of a task from the client
    pub async fn get_task(
        &self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskResult> {
        self.request(schema::ServerRequest::get_task(task_id)).await
    }

    /// Retrieve the result of a completed task from the client
    pub async fn get_task_payload(
        &self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::GetTaskPayloadResult> {
        self.request(schema::ServerRequest::get_task_payload(task_id))
            .await
    }

    /// List tasks with optional pagination from the client
    pub async fn list_tasks(
        &self,
        cursor: impl Into<Option<schema::Cursor>> + Send,
    ) -> Result<schema::ListTasksResult> {
        self.request(schema::ServerRequest::list_tasks(cursor.into()))
            .await
    }

    /// Cancel a task by ID on the client
    pub async fn cancel_task(
        &self,
        task_id: impl Into<String> + Send,
    ) -> Result<schema::CancelTaskResult> {
        self.request(schema::ServerRequest::cancel_task(task_id))
            .await
    }
}
