use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use http::Extensions;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::{self, error::TrySendError};

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
    pub(crate) notification_tx: mpsc::Sender<schema::ClientNotification>,
    /// The current request ID, if this context is handling a request
    pub(crate) request_id: Option<schema::RequestId>,
}

impl ClientCtx {
    /// Create a new `ClientCtx` with the given notification sender
    pub(crate) fn new(notification_tx: mpsc::Sender<schema::ClientNotification>) -> Self {
        Self {
            notification_tx,
            request_id: None,
        }
    }

    /// Send a notification to the server
    pub fn notify(&self, notification: schema::ClientNotification) -> Result<()> {
        self.notification_tx
            .try_send(notification)
            .map_err(|err| notification_send_error(&err))?;
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
    pub(crate) notification_tx: mpsc::Sender<schema::ServerNotification>,
    /// Request handler for making requests to clients
    request_handler: RequestHandler,
    /// The current request ID, if this context is handling a request
    pub(crate) request_id: Option<schema::RequestId>,
    /// Per-request transport extensions.
    extensions: Extensions,
    /// Optional progress token attached to the active request.
    progress_token: Option<schema::ProgressToken>,
    /// Shared progress counter for all clones derived from the same request context.
    progress_counter: Option<Arc<AtomicU64>>,
}

impl ServerCtx {
    /// Create a new ServerCtx with notification channel and transport
    pub(crate) fn new(
        notification_tx: mpsc::Sender<schema::ServerNotification>,
        transport_tx: Option<TransportSink>,
    ) -> Self {
        Self {
            notification_tx,
            request_handler: RequestHandler::new(transport_tx, "srv-req".to_string()),
            request_id: None,
            extensions: Extensions::new(),
            progress_token: None,
            progress_counter: None,
        }
    }

    /// Send a notification to the client
    pub fn notify(&self, notification: schema::ServerNotification) -> Result<()> {
        self.notification_tx
            .try_send(notification)
            .map_err(|err| notification_send_error(&err))?;
        Ok(())
    }

    /// Create a new context with a specific request ID
    pub(crate) fn with_request_id(&self, request_id: schema::RequestId) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx
    }

    /// Create a new context with a specific progress token.
    #[must_use]
    pub fn with_progress_token(&self, token: schema::ProgressToken) -> Self {
        let mut ctx = self.clone();
        ctx.progress_token = Some(token);
        ctx.progress_counter = Some(Arc::new(AtomicU64::new(0)));
        ctx
    }

    /// Return the progress token for this request, if present.
    #[must_use]
    pub fn progress_token(&self) -> Option<&schema::ProgressToken> {
        self.progress_token.as_ref()
    }

    /// Send an informational progress notification for the current request.
    ///
    /// Progress is best-effort: missing tokens and bounded-queue failures are ignored.
    pub fn send_progress(&self, message: &str) {
        let (Some(token), Some(counter)) = (&self.progress_token, &self.progress_counter) else {
            return;
        };
        let progress = counter.fetch_add(1, Ordering::Relaxed) + 1;
        drop(self.notify(schema::ServerNotification::progress(
            token.clone(),
            progress as f64,
            None,
            Some(message.to_owned()),
        )));
    }

    /// Return per-request transport extensions.
    pub fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    /// Create a new context with request-scoped extensions.
    pub(crate) fn with_extensions(&self, extensions: Extensions) -> Self {
        let mut ctx = self.clone();
        ctx.extensions = extensions;
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

    /// Shut down any in-flight client requests tied to this connection.
    pub(crate) fn shutdown_requests(&self) {
        self.request_handler.shutdown();
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

    // --- MCP protocol methods for client interaction ---
    //
    // These methods allow a server to make requests to the connected client.

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

/// Convert a bounded notification queue error into the crate error type.
fn notification_send_error<T>(err: &TrySendError<T>) -> Error {
    match err {
        TrySendError::Full(_) => Error::Transport("Notification queue full".into()),
        TrySendError::Closed(_) => Error::TransportDisconnected,
    }
}
