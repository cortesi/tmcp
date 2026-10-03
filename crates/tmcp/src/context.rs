use std::{
    collections::HashMap,
    future::pending,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use http::Extensions;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use tokio::{
    sync::mpsc::{self, error::TrySendError},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::{
    client::handle_server_request,
    connection::ClientHandler,
    custom::{CustomRequest, check_payload},
    error::{Error, Result},
    request_handler::{
        CancelSender, DEFAULT_TIMEOUT_MS, NO_TIMEOUT_MS, RequestHandler, TransportSink,
        request_body, timeout_millis,
    },
    schema::{self},
};

/// Requests in flight on one connection, each with the token that cancels it.
#[derive(Clone, Default)]
struct InFlight(Arc<Mutex<HashMap<schema::RequestId, CancellationToken>>>);

impl InFlight {
    /// Record that a request has started, and return its cancellation token.
    fn begin(&self, request_id: &schema::RequestId) -> CancellationToken {
        let token = CancellationToken::new();
        self.0
            .lock()
            .expect("in-flight request lock")
            .insert(request_id.clone(), token.clone());
        token
    }

    /// Record that a request has finished, and return whether it was
    /// cancelled.
    ///
    /// The check and the removal are one step under the lock, and
    /// [`Self::cancel`] cancels under the same lock. Thus a cancellation
    /// either comes first and suppresses the response, or comes after and is
    /// ignored.
    fn finish(&self, request_id: &schema::RequestId) -> bool {
        self.0
            .lock()
            .expect("in-flight request lock")
            .remove(request_id)
            .is_some_and(|token| token.is_cancelled())
    }

    /// Cancel one in-flight request.
    ///
    /// Cancellations for requests that are not in flight (already completed,
    /// or never seen) are ignored, so late cancellations cannot grow state
    /// without bound.
    fn cancel(&self, request_id: &schema::RequestId) {
        let in_flight = self.0.lock().expect("in-flight request lock");
        if let Some(token) = in_flight.get(request_id) {
            token.cancel();
        }
    }
}

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
    /// Cancellation of the server request that this context handles.
    cancel: Option<CancellationToken>,
    /// Server requests in flight on this connection.
    in_flight: InFlight,
}

impl ClientCtx {
    /// Create a new `ClientCtx` with the given notification sender
    pub(crate) fn new(notification_tx: mpsc::Sender<schema::ClientNotification>) -> Self {
        Self {
            notification_tx,
            request_id: None,
            cancel: None,
            in_flight: InFlight::default(),
        }
    }

    /// Send a notification to the server
    pub fn notify(&self, notification: schema::ClientNotification) -> Result<()> {
        self.notification_tx
            .try_send(notification)
            .map_err(|err| notification_send_error(&err))?;
        Ok(())
    }

    /// Return the ID of the server request that this context handles.
    #[must_use]
    pub fn request_id(&self) -> Option<&schema::RequestId> {
        self.request_id.as_ref()
    }

    /// Return whether the server cancelled the request that this context
    /// handles.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    /// Wait until the server cancels the request that this context handles.
    ///
    /// A server across a transport cancels a request with
    /// `notifications/cancelled`, and the client then sends no response. A
    /// server in the same process (see [`ServerCtx::in_process`]) cancels a
    /// request when it stops waiting for the response. A context without a
    /// request never resolves this future.
    pub async fn cancelled(&self) {
        match &self.cancel {
            Some(token) => token.cancelled().await,
            None => pending().await,
        }
    }

    /// Create the context of one server request with its cancellation token.
    pub(crate) fn for_request(
        &self,
        request_id: schema::RequestId,
        cancel: CancellationToken,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx.cancel = Some(cancel);
        ctx
    }

    /// Create the context of one server request, and track the request so
    /// that a cancellation notification reaches it.
    pub(crate) fn begin_request(&self, request_id: &schema::RequestId) -> Self {
        let cancel = self.in_flight.begin(request_id);
        self.for_request(request_id.clone(), cancel)
    }

    /// Stop tracking the request of this context, and return whether the
    /// server cancelled it. A cancelled request gets no response.
    pub(crate) fn finish_request(&self) -> bool {
        self.request_id
            .as_ref()
            .is_some_and(|request_id| self.in_flight.finish(request_id))
    }

    /// Cancel one server request that is in flight on this connection.
    pub(crate) fn mark_cancelled(&self, request_id: &schema::RequestId) {
        self.in_flight.cancel(request_id);
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
    /// The client that receives the requests of this server.
    peer: ClientPeer,
    /// The current request ID, if this context is handling a request
    request_id: Option<schema::RequestId>,
    /// Cancellation of the current request, if this context is handling one.
    cancel: Option<CancellationToken>,
    /// Per-request transport extensions.
    extensions: Extensions,
    /// Optional progress token attached to the active request.
    progress_token: Option<schema::ProgressToken>,
    /// Metadata fields the client sent with the active request, other than the
    /// progress token.
    request_meta: Option<Arc<Map<String, Value>>>,
    /// Shared progress counter for all clones derived from the same request
    /// context.
    progress_counter: Option<Arc<AtomicU64>>,
    /// Requests currently being handled on this connection.
    in_flight: InFlight,
    /// Connection input ended or the server stopped this connection.
    connection_closed: CancellationToken,
}

/// The client that receives the requests of a server.
#[derive(Clone)]
enum ClientPeer {
    /// A client across a transport. A notification-only context has no
    /// transport.
    Remote {
        /// The requests of the server to the client.
        requests: RequestHandler,
        /// The control queue of the connection loop, which sends the
        /// cancellation notice of each abandoned custom request. A context
        /// without a connection loop has none.
        cancellations: Option<CancelSender>,
    },
    /// A client handler in the same process as the server.
    InProcess(Arc<InProcessClient>),
}

/// A client handler that a host runs in the same process as a server.
struct InProcessClient {
    /// The handler that answers each server request.
    handler: Arc<dyn ClientHandler>,
    /// The connection context of the handler. It cannot notify the server.
    context: ClientCtx,
    /// Next request ID counter.
    next_request_id: AtomicU64,
}

impl InProcessClient {
    /// Handle one request on its own task, and return its response.
    ///
    /// The handler runs until it finishes. When the caller stops waiting,
    /// because it drops the future or reaches the deadline, the context of the
    /// handler sees cancellation.
    async fn dispatch(
        &self,
        request: schema::Request,
        timeout_override: Option<Duration>,
    ) -> Result<schema::JSONRPCResponse> {
        let next = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let request_id = schema::RequestId::String(format!("srv-req-{next}"));
        let cancel = CancellationToken::new();
        let context = self.context.for_request(request_id.clone(), cancel.clone());
        let handler = Arc::clone(&self.handler);
        let request = schema::JSONRPCRequest {
            jsonrpc: schema::JSONRPC_VERSION.to_string(),
            id: request_id.clone(),
            request,
        };
        let task = tokio::spawn(async move {
            handle_server_request(handler.as_ref(), &context, request).await
        });
        let guard = cancel.drop_guard();
        let timeout_ms = timeout_override.map_or(DEFAULT_TIMEOUT_MS, timeout_millis);
        let joined = if timeout_ms == NO_TIMEOUT_MS {
            task.await
        } else if let Ok(joined) = timeout(Duration::from_millis(timeout_ms), task).await {
            joined
        } else {
            debug!("In-process request {request_id} timed out after {timeout_ms}ms");
            return Err(Error::Timeout {
                request_id: request_id.to_string(),
                timeout_ms,
            });
        };
        guard.disarm();
        joined.map_err(|error| Error::InternalError(format!("client handler failed: {error}")))
    }
}

impl ServerCtx {
    /// Create a context that can emit notifications but cannot request client
    /// actions.
    pub fn notification_only(notification_tx: mpsc::Sender<schema::ServerNotification>) -> Self {
        let requests = RequestHandler::new(None, "srv-req".to_string());
        Self::with_peer(
            notification_tx,
            ClientPeer::Remote {
                requests,
                cancellations: None,
            },
        )
    }

    /// Create the connection context of a server that a host drives in its
    /// own process, without a transport.
    ///
    /// Server notifications go to `notification_tx`. Each request of the
    /// server to its client, standard or custom, goes to `client` on its own
    /// task. The requests keep their deadlines, cancellation, and payload
    /// limits, and the server sees the results and errors that a client across
    /// a transport gives. The [`ClientCtx`] of `client` cannot notify the
    /// server.
    ///
    /// Derive the context of each request with [`Self::for_request`], and
    /// dispatch the request with [`handle_request`](crate::handle_request).
    pub fn in_process(
        notification_tx: mpsc::Sender<schema::ServerNotification>,
        client: impl ClientHandler + 'static,
    ) -> Self {
        // The receiver is dropped, so a notification from the client handler
        // fails with a disconnected transport.
        let (client_notification_tx, _) = mpsc::channel(1);
        let client = InProcessClient {
            handler: Arc::new(client),
            context: ClientCtx::new(client_notification_tx),
            next_request_id: AtomicU64::new(1),
        };
        Self::with_peer(notification_tx, ClientPeer::InProcess(Arc::new(client)))
    }

    /// Create the connection context of a server on a transport.
    ///
    /// The server sends its requests to the client over `transport_tx`. The
    /// connection loop receives the ID of each abandoned custom request from
    /// `cancellations`, and sends its cancellation notice to the client.
    pub(crate) fn connected(
        notification_tx: mpsc::Sender<schema::ServerNotification>,
        transport_tx: TransportSink,
        cancellations: CancelSender,
    ) -> Self {
        let requests = RequestHandler::new(Some(transport_tx), "srv-req".to_string());
        Self::with_peer(
            notification_tx,
            ClientPeer::Remote {
                requests,
                cancellations: Some(cancellations),
            },
        )
    }

    /// Create a connection context for one client.
    fn with_peer(
        notification_tx: mpsc::Sender<schema::ServerNotification>,
        peer: ClientPeer,
    ) -> Self {
        Self {
            notification_tx,
            peer,
            request_id: None,
            cancel: None,
            extensions: Extensions::new(),
            progress_token: None,
            request_meta: None,
            progress_counter: None,
            in_flight: InFlight::default(),
            connection_closed: CancellationToken::new(),
        }
    }

    /// Send a notification to the client
    pub fn notify(&self, notification: schema::ServerNotification) -> Result<()> {
        self.notification_tx
            .try_send(notification)
            .map_err(|err| notification_send_error(&err))?;
        Ok(())
    }

    /// Return the ID of the current request.
    #[must_use]
    pub fn request_id(&self) -> Option<&schema::RequestId> {
        self.request_id.as_ref()
    }

    /// Create the context of one request: its ID and the token that cancels
    /// it.
    ///
    /// The new context keeps the notification channel, the client, the
    /// connection, and the extensions of this context. It drops the progress
    /// token and the request metadata of an earlier request. A host that
    /// drives a server handler without a transport uses it with
    /// [`handle_request`](crate::handle_request): a cancelled token cancels
    /// the request, as `notifications/cancelled` from a client does.
    #[must_use]
    pub fn for_request(&self, request_id: schema::RequestId, cancel: CancellationToken) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx.cancel = Some(cancel);
        ctx.progress_token = None;
        ctx.progress_counter = None;
        ctx.request_meta = None;
        ctx
    }

    /// Create a new context with a specific request ID that cannot be
    /// cancelled.
    pub(crate) fn with_request_id(&self, request_id: schema::RequestId) -> Self {
        let mut ctx = self.clone();
        ctx.request_id = Some(request_id);
        ctx.cancel = None;
        ctx
    }

    /// Return whether the current request has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    /// Wait until the current request is cancelled.
    ///
    /// A client cancels a request with `notifications/cancelled`. A host
    /// without a transport cancels the token of [`Self::for_request`]. A
    /// context without a request never resolves this future.
    pub async fn cancelled(&self) {
        match &self.cancel {
            Some(token) => token.cancelled().await,
            None => pending().await,
        }
    }

    /// Wait until connection input ends or the server stops this connection.
    ///
    /// Stream EOF includes a client's write-side half-close. Handlers may use
    /// this signal to abandon work that needs a connected client. It does not
    /// mark requests cancelled or suppress responses: handlers that ignore it
    /// can still finish and respond after EOF. Notification-only and
    /// in-process contexts have no connection and never resolve this future.
    pub async fn connection_closed(&self) {
        self.connection_closed.cancelled().await;
    }

    /// Create a new context with a specific progress token.
    #[must_use]
    pub fn with_progress_token(&self, token: schema::ProgressToken) -> Self {
        let mut ctx = self.clone();
        ctx.progress_token = Some(token);
        ctx.progress_counter = Some(Arc::new(AtomicU64::new(0)));
        ctx
    }

    /// Create a new context carrying the request's other `_meta` fields.
    ///
    /// A protocol extension reads its own key from this map. The progress
    /// token stays on [`Self::progress_token`].
    #[must_use]
    pub fn with_request_meta(&self, meta: Map<String, Value>) -> Self {
        let mut ctx = self.clone();
        ctx.request_meta = Some(Arc::new(meta));
        ctx
    }

    /// Return one `_meta` field the client sent with the active request.
    #[must_use]
    pub fn request_meta(&self, key: &str) -> Option<&Value> {
        self.request_meta.as_ref()?.get(key)
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
    ///
    /// A transport sets the extensions of each request, such as the parts of
    /// an HTTP request. A host without a transport can set its own values.
    #[must_use]
    pub fn with_extensions(&self, extensions: Extensions) -> Self {
        let mut ctx = self.clone();
        ctx.extensions = extensions;
        ctx
    }

    /// Create the context of one request, and track the request so that a
    /// cancellation notification reaches it.
    pub(crate) fn begin_request(&self, request_id: &schema::RequestId) -> Self {
        let cancel = self.in_flight.begin(request_id);
        self.for_request(request_id.clone(), cancel)
    }

    /// Stop tracking the request of this context, and return whether the
    /// client cancelled it. A cancelled request gets no response.
    pub(crate) fn finish_request(&self) -> bool {
        self.request_id
            .as_ref()
            .is_some_and(|request_id| self.in_flight.finish(request_id))
    }

    /// Cancel one request that is in flight on this connection.
    ///
    /// Cancellations for requests that are not in flight (already completed,
    /// or never seen) are ignored.
    pub(crate) fn mark_cancelled(&self, request_id: &schema::RequestId) {
        self.in_flight.cancel(request_id);
    }

    /// Send a request to the client and wait for response
    async fn request<T>(&self, request: schema::ServerRequest) -> Result<T>
    where
        T: DeserializeOwned + Send + 'static,
    {
        match &self.peer {
            ClientPeer::Remote { requests, .. } => requests.request(request).await,
            ClientPeer::InProcess(client) => {
                let response = client.dispatch(request_body(&request)?, None).await?;
                RequestHandler::decode_response(request.method(), response)
            }
        }
    }

    /// Send a custom request to the client and wait for its result.
    ///
    /// The client answers with
    /// [`ClientHandler::custom_request`](crate::ClientHandler::custom_request),
    /// and the result decodes into `T`. A client without an answer for the
    /// method returns [`Error::MethodNotFound`].
    ///
    /// The deadline of the request starts before its send, so a slow
    /// transport counts toward it. The client sees the request cancelled when
    /// the server stops waiting: when the caller drops the future, for
    /// example in `tokio::select!`, also during the send, or when the
    /// deadline of the request passes ([`Error::Timeout`]). The cancellation
    /// notice does not use the bounded notification queue, so a full queue
    /// does not drop it. When the connection ends, the request fails with
    /// [`Error::TransportDisconnected`].
    ///
    /// The cancellation of the current request does not cancel the custom
    /// request. Select on [`Self::cancelled`] for that.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PayloadTooLarge`] when the params or the result are
    /// larger than the payload limit of the request. The server does not send
    /// params over the limit.
    pub async fn send_request<T>(&self, request: CustomRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let (body, timeout_override, payload_limit) = request.prepare()?;
        let method = body.method.clone();
        let response = match &self.peer {
            ClientPeer::Remote {
                requests,
                cancellations,
            } => {
                let (_, pending) = requests
                    .request_raw_pending(body, timeout_override, cancellations.clone())
                    .await?;
                pending.await?
            }
            ClientPeer::InProcess(client) => client.dispatch(body, timeout_override).await?,
        };
        if let schema::JSONRPCResponse::Result(result) = &response {
            check_payload(&method, &result.result, payload_limit)?;
        }
        RequestHandler::decode_response(&method, response)
    }

    /// Handle a response from the client
    pub(crate) async fn handle_client_response(&self, response: schema::JSONRPCResponse) {
        if let ClientPeer::Remote { requests, .. } = &self.peer {
            requests.handle_response(response).await;
        }
    }

    /// Notify handlers of connection closure and stop server-to-client
    /// requests.
    pub(crate) fn shutdown_requests(&self) {
        self.connection_closed.cancel();
        if let ClientPeer::Remote { requests, .. } = &self.peer {
            requests.shutdown();
        }
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
    use std::{
        sync::{Barrier, mpsc::channel},
        thread,
    };

    use tokio::{
        sync::mpsc,
        task::yield_now,
        time::{Duration, timeout},
    };

    use super::*;
    use crate::schema::{LoggingLevel, ProgressToken, ServerNotification};

    #[test]
    fn progress_without_token_is_noop() {
        let (notification_tx, mut notification_rx) = mpsc::channel(1);
        let ctx = ServerCtx::notification_only(notification_tx);

        ctx.send_progress("hidden");

        assert!(notification_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn progress_with_total_uses_request_token_and_counter() {
        let (notification_tx, mut notification_rx) = mpsc::channel(2);
        let ctx = ServerCtx::notification_only(notification_tx)
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
        let ctx = ServerCtx::notification_only(notification_tx);

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

    #[tokio::test]
    async fn cancellation_wakes_current_and_late_waiters() {
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let connection = ServerCtx::notification_only(notification_tx);
        let request_id = schema::RequestId::String("waiters".to_owned());
        let ctx = connection.begin_request(&request_id);

        timeout(Duration::from_millis(100), async {
            tokio::join!(ctx.cancelled(), ctx.cancelled(), async {
                yield_now().await;
                connection.mark_cancelled(&request_id);
            });
            ctx.cancelled().await;
        })
        .await
        .expect("all cancellation waiters wake");
        assert!(ctx.is_cancelled());
        assert!(ctx.finish_request());
    }

    #[test]
    fn late_cancellation_is_ignored() {
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let connection = ServerCtx::notification_only(notification_tx);
        let request_id = schema::RequestId::Number(7);
        let ctx = connection.begin_request(&request_id);
        assert!(!ctx.finish_request());

        connection.mark_cancelled(&request_id);

        assert!(!ctx.is_cancelled());
        assert!(connection.in_flight.0.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn a_request_context_takes_the_cancellation_of_the_host() {
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let connection = ServerCtx::notification_only(notification_tx)
            .with_progress_token(ProgressToken::String("earlier".to_owned()));
        let cancel = CancellationToken::new();
        let ctx = connection.for_request(schema::RequestId::Number(3), cancel.clone());

        assert_eq!(ctx.request_id(), Some(&schema::RequestId::Number(3)));
        assert!(ctx.progress_token().is_none());
        assert!(!ctx.is_cancelled());
        cancel.cancel();
        timeout(Duration::from_millis(100), ctx.cancelled())
            .await
            .expect("the host token cancels the request");
        assert!(ctx.is_cancelled());
        assert!(!connection.is_cancelled());
    }

    #[tokio::test]
    async fn client_cancellation_reaches_only_its_request() {
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let connection = ClientCtx::new(notification_tx);
        let first = connection.begin_request(&schema::RequestId::Number(1));
        let second = connection.begin_request(&schema::RequestId::Number(2));

        connection.mark_cancelled(&schema::RequestId::Number(1));

        timeout(Duration::from_millis(100), first.cancelled())
            .await
            .expect("the cancelled request wakes");
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        assert_eq!(second.request_id(), Some(&schema::RequestId::Number(2)));
        assert!(first.finish_request());
        assert!(!second.finish_request());
    }

    #[tokio::test]
    async fn connection_shutdown_wakes_current_and_late_waiters_without_cancelling_requests() {
        let (tx, _rx) = mpsc::channel(1);
        let id = schema::RequestId::String("pending".into());
        let ctx = ServerCtx::notification_only(tx).begin_request(&id);
        let stop = ctx.clone();
        timeout(Duration::from_secs(1), async {
            tokio::join!(ctx.connection_closed(), ctx.connection_closed(), async {
                yield_now().await;
                stop.shutdown_requests();
            });
            ctx.connection_closed().await;
        })
        .await
        .expect("all connection waiters wake");
        assert!(!ctx.is_cancelled());
        assert!(!ctx.finish_request());
    }

    /// The rounds of the race between a cancellation and the finish of a
    /// request.
    const RACE_ROUNDS: i64 = 20_000;

    #[test]
    fn a_cancellation_and_the_finish_of_a_request_have_one_order() {
        // A response is suppressed exactly when the cancellation reached the
        // handler. Each round races one cancellation against the finish of
        // its request on two threads.
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        let connection = ServerCtx::notification_only(notification_tx);
        let (ids_tx, ids_rx) = channel::<schema::RequestId>();
        let barrier = Arc::new(Barrier::new(2));
        let canceller = thread::spawn({
            let connection = connection.clone();
            let barrier = Arc::clone(&barrier);
            move || {
                for request_id in ids_rx {
                    barrier.wait();
                    connection.mark_cancelled(&request_id);
                    barrier.wait();
                }
            }
        });

        let mut suppressed_rounds = 0;
        for round in 0..RACE_ROUNDS {
            let request_id = schema::RequestId::Number(round);
            let ctx = connection.begin_request(&request_id);
            ids_tx.send(request_id).expect("canceller");
            barrier.wait();
            let suppressed = ctx.finish_request();
            barrier.wait();
            assert_eq!(
                suppressed,
                ctx.is_cancelled(),
                "round {round}: the response and the cancellation disagree"
            );
            suppressed_rounds += usize::from(suppressed);
        }
        drop(ids_tx);
        canceller.join().expect("canceller");
        // Both orders occur, so the rounds race.
        assert!(suppressed_rounds > 0);
        assert!(suppressed_rounds < RACE_ROUNDS as usize);
    }

    #[test]
    fn request_meta_keeps_extension_fields_beside_the_progress_token() {
        let (tx, _rx) = mpsc::channel(1);
        let ctx = ServerCtx::notification_only(tx);
        assert_eq!(ctx.request_meta("verber/call"), None);

        let mut meta = Map::new();
        meta.insert(
            "verber/call".to_owned(),
            serde_json::json!({ "version": 1 }),
        );
        let ctx = ctx
            .with_progress_token(schema::ProgressToken::String("token".to_owned()))
            .with_request_meta(meta);

        assert_eq!(
            ctx.request_meta("verber/call"),
            Some(&serde_json::json!({ "version": 1 }))
        );
        assert_eq!(ctx.request_meta("other"), None);
        assert!(matches!(
            ctx.progress_token(),
            Some(schema::ProgressToken::String(token)) if token == "token"
        ));
    }
}
