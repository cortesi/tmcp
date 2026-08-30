use std::{
    collections::{HashMap, HashSet},
    future::pending,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use http::Extensions;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{
    Notify,
    mpsc::{self, error::TrySendError},
};

use crate::{
    error::{Error, Result},
    request_handler::{RequestHandler, TransportSink},
    schema::{self},
};

/// Context provided to `ClientHandler` implementations for interacting with the
/// server
///
/// This context is only valid for the duration of a single method call and
/// should not be stored or used outside of that scope. The Clone implementation
/// is for internal framework use only.
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

/// Context provided to `ServerHandler` implementations for interacting with
/// clients
///
/// The framework derives a request-scoped clone for each handler invocation;
/// the underlying channels live for the duration of the connection. Handlers
/// may clone the context into spawned tasks (for example to emit progress),
/// but it stops working once its connection closes.
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
    /// Shared progress counter for all clones derived from the same request
    /// context.
    progress_counter: Option<Arc<AtomicU64>>,
    /// Request IDs cancelled by the client on this connection.
    cancelled_requests: Arc<Mutex<HashSet<schema::RequestId>>>,
    /// Notifiers for request-scoped cancellation waiters.
    cancellation_notifiers: Arc<Mutex<HashMap<schema::RequestId, Arc<Notify>>>>,
    /// Request IDs currently being handled on this connection.
    ///
    /// Cancellation notifications for requests that are not in flight are
    /// ignored, so late cancellations cannot grow state without bound.
    in_flight: Arc<Mutex<HashSet<schema::RequestId>>>,
}

impl ServerCtx {
    /// Create a context that can emit notifications but cannot request client
    /// actions.
    pub fn notification_only(notification_tx: mpsc::Sender<schema::ServerNotification>) -> Self {
        Self::new(notification_tx, None)
    }

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
            cancelled_requests: Arc::new(Mutex::new(HashSet::new())),
            cancellation_notifiers: Arc::new(Mutex::new(HashMap::new())),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
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

    /// Return whether the current request has been cancelled by the client.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        let Some(request_id) = &self.request_id else {
            return false;
        };
        self.is_request_cancelled(request_id)
    }

    /// Return whether a specific request has been cancelled by the client.
    pub(crate) fn is_request_cancelled(&self, request_id: &schema::RequestId) -> bool {
        self.cancelled_requests
            .lock()
            .expect("cancelled request lock")
            .contains(request_id)
    }

    /// Wait until the current request is cancelled by the client.
    pub async fn cancelled(&self) {
        let Some(request_id) = &self.request_id else {
            pending::<()>().await;
            return;
        };
        if self.is_cancelled() {
            return;
        }
        let notifier = {
            let mut notifiers = self
                .cancellation_notifiers
                .lock()
                .expect("cancellation notifier lock");
            Arc::clone(
                notifiers
                    .entry(request_id.clone())
                    .or_insert_with(|| Arc::new(Notify::new())),
            )
        };
        notifier.notified().await;
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
    /// Progress is best-effort: missing tokens and bounded-queue failures are
    /// ignored.
    pub fn send_progress(&self, message: &str) {
        self.send_progress_update(None, Some(message));
    }

    /// Send an informational progress notification with a known total.
    ///
    /// Progress is best-effort: missing tokens and bounded-queue failures are
    /// ignored.
    pub fn send_progress_with_total(&self, message: &str, total: f64) {
        self.send_progress_update(Some(total), Some(message));
    }

    /// Send a progress notification for the current request.
    ///
    /// Each call increments the request-local progress counter. Missing
    /// progress tokens and bounded-queue failures are ignored because MCP
    /// progress is advisory.
    pub fn send_progress_update(&self, total: Option<f64>, message: Option<&str>) {
        let (Some(token), Some(counter)) = (&self.progress_token, &self.progress_counter) else {
            return;
        };
        let progress = counter.fetch_add(1, Ordering::Relaxed) + 1;
        drop(self.notify(schema::ServerNotification::progress(
            token.clone(),
            progress as f64,
            total,
            message.map(str::to_owned),
        )));
    }

    /// Send a structured logging notification without a logger name.
    ///
    /// Unlike progress, logging is not request-scoped. Serialization and
    /// notification queue failures are returned to the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if `data` cannot be serialized or the notification
    /// queue is full.
    pub fn send_log(&self, level: schema::LoggingLevel, data: impl Serialize) -> Result<()> {
        self.send_log_inner(level, None, data)
    }

    /// Send a structured logging notification with a logger name.
    ///
    /// # Errors
    ///
    /// Returns an error if `data` cannot be serialized or the notification
    /// queue is full.
    pub fn send_log_from(
        &self,
        level: schema::LoggingLevel,
        logger: impl Into<String>,
        data: impl Serialize,
    ) -> Result<()> {
        self.send_log_inner(level, Some(logger.into()), data)
    }

    /// Serialize and emit one structured logging notification.
    fn send_log_inner(
        &self,
        level: schema::LoggingLevel,
        logger: Option<String>,
        data: impl Serialize,
    ) -> Result<()> {
        let data = serde_json::to_value(data).map_err(|err| Error::JsonParse {
            message: err.to_string(),
        })?;
        self.notify(schema::ServerNotification::logging_message(
            level, logger, data,
        ))
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

    /// Record that a request has started being handled.
    pub(crate) fn begin_request(&self, request_id: &schema::RequestId) {
        self.in_flight
            .lock()
            .expect("in-flight request lock")
            .insert(request_id.clone());
    }

    /// Record that a request has finished, clearing its cancellation state.
    pub(crate) fn end_request(&self, request_id: &schema::RequestId) {
        self.in_flight
            .lock()
            .expect("in-flight request lock")
            .remove(request_id);
        self.cancelled_requests
            .lock()
            .expect("cancelled request lock")
            .remove(request_id);
        self.cancellation_notifiers
            .lock()
            .expect("cancellation notifier lock")
            .remove(request_id);
    }

    /// Mark one in-flight request as cancelled and wake any waiters.
    ///
    /// Cancellations for requests that are not in flight (already completed,
    /// or never seen) are ignored.
    pub(crate) fn mark_cancelled(&self, request_id: &schema::RequestId) {
        if !self
            .in_flight
            .lock()
            .expect("in-flight request lock")
            .contains(request_id)
        {
            return;
        }
        self.cancelled_requests
            .lock()
            .expect("cancelled request lock")
            .insert(request_id.clone());
        let notifier = self
            .cancellation_notifiers
            .lock()
            .expect("cancellation notifier lock")
            .get(request_id)
            .cloned();
        if let Some(notifier) = notifier {
            notifier.notify_waiters();
        }
    }

    /// Send a request to the client and wait for response
    async fn request<T>(&self, request: schema::ServerRequest) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static,
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

    /// Send a ping request to the client and wait for the response.
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

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::schema::{LoggingLevel, ProgressToken, ServerNotification};

    #[test]
    fn progress_without_token_is_noop() {
        let (notification_tx, mut notification_rx) = mpsc::channel(1);
        let ctx = ServerCtx::new(notification_tx, None);

        ctx.send_progress("hidden");

        assert!(notification_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn progress_with_total_uses_request_token_and_counter() {
        let (notification_tx, mut notification_rx) = mpsc::channel(2);
        let ctx = ServerCtx::new(notification_tx, None)
            .with_progress_token(ProgressToken::String("progress-1".to_owned()));

        ctx.send_progress_with_total("halfway", 2.0);

        let notification = notification_rx.recv().await.expect("notification");
        let ServerNotification::Progress {
            progress_token,
            progress,
            total,
            message,
            _meta: _,
        } = notification
        else {
            panic!("expected progress notification");
        };
        assert!(matches!(progress_token, ProgressToken::String(token) if token == "progress-1"));
        assert_eq!(progress, 1.0);
        assert_eq!(total, Some(2.0));
        assert_eq!(message.as_deref(), Some("halfway"));
    }

    #[tokio::test]
    async fn send_log_from_emits_structured_logging_message() {
        let (notification_tx, mut notification_rx) = mpsc::channel(2);
        let ctx = ServerCtx::new(notification_tx, None);

        ctx.send_log_from(
            LoggingLevel::Info,
            "test",
            serde_json::json!({ "event": "ready" }),
        )
        .expect("send log");

        let notification = notification_rx.recv().await.expect("notification");
        let ServerNotification::LoggingMessage {
            level,
            logger,
            data,
            _meta: _,
        } = notification
        else {
            panic!("expected logging notification");
        };
        assert_eq!(level, LoggingLevel::Info);
        assert_eq!(logger.as_deref(), Some("test"));
        assert_eq!(data["event"], "ready");
    }

    #[tokio::test]
    async fn notification_only_context_has_no_request_transport() {
        let (notification_tx, mut notification_rx) = mpsc::channel(2);
        let ctx = ServerCtx::notification_only(notification_tx);

        ctx.notify(ServerNotification::logging_message(
            LoggingLevel::Info,
            None,
            serde_json::json!({"event": "ready"}),
        ))
        .expect("send notification");
        assert!(notification_rx.recv().await.is_some());

        assert!(matches!(
            ctx.ping().await,
            Err(Error::Transport(message)) if message == "Not connected"
        ));
    }
}
