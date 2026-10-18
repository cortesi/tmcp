//! Request/response routing and tracking.
//!
//! This module provides the [`RequestHandler`] which correlates outgoing
//! requests with incoming responses, handles timeouts, and supports
//! cancellation.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use dashmap::DashMap;
use futures::{SinkExt, stream::SplitSink};
use serde::de::DeserializeOwned;
use tokio::{
    sync::{Mutex as TokioMutex, mpsc, oneshot},
    time::{Instant, timeout_at},
};

use crate::{
    error::{Error, Result},
    jsonrpc::request_params,
    schema::{
        AUTHORIZATION_FAILED, ClientRequest, INVALID_PARAMS, JSONRPC_VERSION, JSONRPCMessage,
        JSONRPCNotification, JSONRPCRequest, JSONRPCResponse, METHOD_NOT_FOUND, Notification,
        NotificationParams, Request, RequestId, ServerRequest,
    },
    transport::TransportStream,
};

/// Default request timeout in milliseconds (30 seconds).
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Timeout value that disables the deadline and waits for the peer or the
/// transport.
pub const NO_TIMEOUT_MS: u64 = 0;

/// Return the deadline of a request in milliseconds.
///
/// A zero duration disables the deadline. A shorter duration than one
/// millisecond rounds up to one millisecond, so that it keeps a deadline.
pub fn timeout_millis(timeout: Duration) -> u64 {
    if timeout.is_zero() {
        NO_TIMEOUT_MS
    } else {
        u64::try_from(timeout.as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }
}

/// Transport sink type used by the request handler.
pub type TransportSink = Arc<TokioMutex<SplitSink<Box<dyn TransportStream>, JSONRPCMessage>>>;

/// The sender of the cancellation notices of a connection.
///
/// Each notice is the ID of a request that the sender abandoned. The queue is
/// unbounded, so a notice never waits for capacity and is never dropped. It
/// holds at most one notice for each abandoned request.
pub type CancelSender = mpsc::UnboundedSender<RequestId>;

/// Metadata for a pending request.
struct PendingRequest {
    /// Channel to send the response back to the caller.
    response_tx: oneshot::Sender<JSONRPCResponse>,
}

/// A typed response future for an outbound JSON-RPC request.
///
/// The deadline of the request started before its send. A first poll after
/// the deadline returns a response that has arrived, or fails with
/// [`Error::Timeout`] at once. When the future drops before the response
/// arrives, the request releases its pending slot.
pub struct Pending<Res> {
    /// Future waiting for and decoding the response. It owns the request.
    inner: Pin<Box<dyn Future<Output = Result<Res>> + Send + 'static>>,
}

impl<Res> Future for Pending<Res> {
    type Output = Result<Res>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

/// One outbound request, from before its send until its response.
///
/// The request owns its pending slot, its absolute deadline, and its
/// cancellation notice through the send and the wait. A request that drops
/// before it settles, because the caller dropped it or its deadline passed,
/// releases its slot and sends its cancellation notice.
struct OutboundRequest {
    /// Shared handler that owns the pending slot.
    handler: RequestHandler,
    /// The ID of the request.
    id: RequestId,
    /// The deadline and its length in milliseconds, or `None` without a
    /// deadline.
    deadline: Option<(Instant, u64)>,
    /// Where to send the cancellation notice, or `None` when the request has
    /// no notice or has settled.
    cancel: Option<CancelSender>,
}

impl OutboundRequest {
    /// Store the pending slot of a new request and start its deadline.
    fn start(
        handler: &RequestHandler,
        timeout_ms: u64,
        cancel: Option<CancelSender>,
    ) -> (Self, oneshot::Receiver<JSONRPCResponse>) {
        let id = handler.next_request_id();
        let (response_tx, response_rx) = oneshot::channel();
        handler
            .inner
            .pending_requests
            .insert(id.clone(), PendingRequest { response_tx });
        tracing::debug!(
            "Stored pending request with ID: {}, total pending: {}",
            id,
            handler.inner.pending_requests.len()
        );
        // A deadline past the range of `Instant` is no deadline in practice.
        let deadline = (timeout_ms != NO_TIMEOUT_MS)
            .then(|| Instant::now().checked_add(Duration::from_millis(timeout_ms)))
            .flatten()
            .map(|deadline| (deadline, timeout_ms));
        let request = Self {
            handler: handler.clone(),
            id,
            deadline,
            cancel,
        };
        (request, response_rx)
    }

    /// Run `future` until the deadline of the request.
    async fn before_deadline<T>(&self, future: impl Future<Output = T>) -> Result<T> {
        // Without a deadline, the caller accepts an unbounded wait, such as a
        // tool that blocks on a human decision. Transport shutdown still wakes
        // the pending request.
        let Some((deadline, timeout_ms)) = self.deadline else {
            return Ok(future.await);
        };
        timeout_at(deadline, future).await.map_err(|_| {
            tracing::warn!("Request {} timed out after {}ms", self.id, timeout_ms);
            Error::Timeout {
                request_id: self.id.to_string(),
                timeout_ms,
            }
        })
    }

    /// Send the request over the transport before its deadline.
    async fn send(&mut self, request: Request) -> Result<()> {
        tracing::info!(
            "Sending request with ID: {} method: {}",
            self.id,
            request.method
        );
        let message = JSONRPCMessage::Request(JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: self.id.clone(),
            request,
        });
        let sent = self
            .before_deadline(self.handler.send_message(message))
            .await?;
        if sent.is_err() {
            // The transport failed, so the peer has no request to cancel.
            self.cancel = None;
        }
        sent
    }

    /// Wait for the response until the deadline.
    async fn response(
        mut self,
        response_rx: oneshot::Receiver<JSONRPCResponse>,
    ) -> Result<JSONRPCResponse> {
        let received = self.before_deadline(response_rx).await?;
        self.cancel = None;
        received.map_err(|_recv_error| {
            if self.handler.inner.shutting_down.load(Ordering::Acquire) {
                Error::TransportDisconnected
            } else {
                Error::Protocol("Response channel closed unexpectedly".to_string())
            }
        })
    }
}

impl Drop for OutboundRequest {
    fn drop(&mut self) {
        // Response dispatch removes the slot before it delivers the response.
        // Only a request that still owned its unresolved slot was abandoned.
        if self
            .handler
            .inner
            .pending_requests
            .remove(&self.id)
            .is_none()
        {
            return;
        }
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        // The request of a closed connection needs no notice.
        if self.handler.inner.shutting_down.load(Ordering::Acquire) {
            return;
        }
        if cancel.send(self.id.clone()).is_err() {
            tracing::debug!(
                "The connection closed before the cancellation of request {}",
                self.id
            );
        }
    }
}

/// Common request/response handling functionality shared between Client and
/// ServerCtx.
///
/// The `RequestHandler` is responsible for:
/// - Correlating outgoing requests with incoming responses via request IDs
/// - Enforcing request timeouts to prevent resource leaks
/// - Supporting request cancellation
/// - Cleaning up stale pending requests
#[derive(Clone)]
pub struct RequestHandler {
    /// Shared mutable state for outbound requests and inbound responses.
    inner: Arc<RequestHandlerInner>,
}

/// Shared request-handler state stored behind [`RequestHandler`].
struct RequestHandlerInner {
    /// Transport sink for sending messages.
    transport_tx: StdMutex<Option<TransportSink>>,
    /// Pending requests waiting for responses, keyed by request ID.
    pending_requests: DashMap<RequestId, PendingRequest>,
    /// Next request ID counter.
    next_request_id: AtomicU64,
    /// Prefix for request IDs (e.g., "req" for client, "srv-req" for server).
    id_prefix: String,
    /// Request timeout in milliseconds.
    timeout_ms: AtomicU64,
    /// Connection control queue for notices about abandoned requests.
    cancellations: StdMutex<Option<CancelSender>>,
    /// Tracks whether the current transport is shutting down.
    shutting_down: AtomicBool,
}

impl RequestHandler {
    /// Create a new RequestHandler with default timeout.
    pub fn new(transport_tx: Option<TransportSink>, id_prefix: String) -> Self {
        Self {
            inner: Arc::new(RequestHandlerInner {
                transport_tx: StdMutex::new(transport_tx),
                pending_requests: DashMap::new(),
                next_request_id: AtomicU64::new(1),
                id_prefix,
                timeout_ms: AtomicU64::new(DEFAULT_TIMEOUT_MS),
                cancellations: StdMutex::new(None),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    /// Set the transport sink (used when connecting).
    pub fn set_transport(&self, transport_tx: TransportSink) {
        if let Ok(mut lock) = self.inner.transport_tx.lock() {
            *lock = Some(transport_tx);
        }
        self.inner.shutting_down.store(false, Ordering::Release);
    }

    /// Set the connection control queue for abandoned-request notices.
    pub(crate) fn set_cancellations(&self, cancellations: Option<CancelSender>) {
        if let Ok(mut lock) = self.inner.cancellations.lock() {
            *lock = cancellations;
        }
    }

    /// Set the default request timeout in milliseconds for this connection.
    ///
    /// Affects every request issued through this handler and its clones.
    pub fn set_timeout(&self, timeout_ms: u64) {
        self.inner.timeout_ms.store(timeout_ms, Ordering::Relaxed);
    }

    /// Shut down the current transport and wake any pending requests
    /// immediately.
    pub fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        if let Ok(mut lock) = self.inner.transport_tx.lock() {
            *lock = None;
        }
        self.set_cancellations(None);
        self.inner.pending_requests.clear();
    }

    /// Returns whether the handler has an active transport.
    pub(crate) fn is_connected(&self) -> bool {
        let has_transport = self
            .inner
            .transport_tx
            .lock()
            .is_ok_and(|transport| transport.is_some());
        has_transport && !self.inner.shutting_down.load(Ordering::Acquire)
    }

    /// Send a request and wait for response with timeout and cancellation
    /// support.
    pub async fn request<Req, Res>(&self, request: Req) -> Result<Res>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        let (_, pending) = self.request_pending(request).await?;
        pending.await
    }

    /// Send a request with a per-request timeout and wait for the response.
    pub async fn request_with_timeout<Req, Res>(
        &self,
        request: Req,
        timeout: Duration,
    ) -> Result<Res>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        let (_, pending) = self
            .request_pending_with_timeout(request, Some(timeout))
            .await?;
        pending.await
    }

    /// Send a request and return its id plus a typed response future.
    pub async fn request_pending<Req, Res>(&self, request: Req) -> Result<(RequestId, Pending<Res>)>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        self.request_pending_with_timeout(request, None).await
    }

    /// Send a request, optionally overriding the connection's default timeout
    /// for this request only.
    pub async fn request_pending_with_timeout<Req, Res>(
        &self,
        request: Req,
        timeout_override: Option<Duration>,
    ) -> Result<(RequestId, Pending<Res>)>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        let request = request_body(&request)?;
        let method = request.method.clone();
        let cancellations = self
            .inner
            .cancellations
            .lock()
            .ok()
            .and_then(|sender| sender.clone());
        self.send_pending(request, timeout_override, cancellations, move |response| {
            Self::decode_response(&method, response)
        })
        .await
    }

    /// Send a request body, optionally overriding the connection's default
    /// timeout, and return its id plus a future of the raw response.
    ///
    /// When the caller abandons the request, because it drops the request
    /// during the send or the wait, or the deadline passes, the request sends
    /// its ID to `cancel`.
    pub(crate) async fn request_raw_pending(
        &self,
        request: Request,
        timeout_override: Option<Duration>,
        cancel: Option<CancelSender>,
    ) -> Result<(RequestId, Pending<JSONRPCResponse>)> {
        self.send_pending(request, timeout_override, cancel, Ok)
            .await
    }

    /// Send a request body and return its id plus a future that finishes its
    /// response with `finish`.
    ///
    /// The request starts its deadline before the send, so a blocked
    /// transport counts toward the deadline.
    async fn send_pending<Res, F>(
        &self,
        request: Request,
        timeout_override: Option<Duration>,
        cancel: Option<CancelSender>,
        finish: F,
    ) -> Result<(RequestId, Pending<Res>)>
    where
        Res: Send + 'static,
        F: FnOnce(JSONRPCResponse) -> Result<Res> + Send + 'static,
    {
        let timeout_ms = timeout_override.map_or_else(
            || self.inner.timeout_ms.load(Ordering::Relaxed),
            timeout_millis,
        );
        let (mut outbound, response_rx) = OutboundRequest::start(self, timeout_ms, cancel);
        let id = outbound.id.clone();
        outbound.send(request).await?;
        let inner = Box::pin(async move { finish(outbound.response(response_rx).await?) });
        Ok((id, Pending { inner }))
    }

    /// Send a message through the transport.
    pub async fn send_message(&self, message: JSONRPCMessage) -> Result<()> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(Error::TransportDisconnected);
        }

        // Lock the transport option
        let transport = {
            let lock = self
                .inner
                .transport_tx
                .lock()
                .map_err(|_| Error::InternalError("Failed to lock transport".into()))?;
            lock.clone()
        };

        if let Some(transport_tx) = transport {
            let mut tx = transport_tx.lock().await;
            tx.send(message).await?;
            Ok(())
        } else {
            Err(Error::Transport("Not connected".to_string()))
        }
    }

    /// Send a notification.
    pub async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        let notification_params = params.map(|v| NotificationParams {
            _meta: None,
            other: match v {
                serde_json::Value::Object(map) => map,
                _ => serde_json::Map::new(),
            },
        });

        let notification = JSONRPCNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            notification: Notification {
                method: method.to_string(),
                params: notification_params,
            },
        };

        self.send_message(JSONRPCMessage::Notification(notification))
            .await
    }

    /// Generate the next request ID.
    fn next_request_id(&self) -> RequestId {
        let id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
        RequestId::String(format!("{}-{}", self.inner.id_prefix, id))
    }

    /// Decode a response payload into the expected result type, as a client
    /// receives it.
    pub(crate) fn decode_response<Res>(method: &str, response: JSONRPCResponse) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        match response {
            JSONRPCResponse::Result(response) => {
                let mut result_value = response.result.other;
                if let Some(meta) = response.result._meta {
                    result_value.insert("_meta".to_string(), serde_json::Value::Object(meta));
                }

                serde_json::from_value(serde_json::Value::Object(result_value))
                    .map_err(|e| Error::Protocol(format!("Failed to deserialize response: {e}")))
            }
            JSONRPCResponse::Error(error) => match error.error.code {
                METHOD_NOT_FOUND => Err(Error::MethodNotFound(error.error.message)),
                INVALID_PARAMS => Err(Error::InvalidParams(format!(
                    "{}: {}",
                    method, error.error.message
                ))),
                AUTHORIZATION_FAILED => Err(Error::AuthorizationFailed(error.error.message)),
                _ => Err(Error::Protocol(format!(
                    "JSON-RPC error {}: {}",
                    error.error.code, error.error.message
                ))),
            },
        }
    }

    /// Handle a response from the remote side.
    ///
    /// This method handles both string and numeric request IDs.
    pub async fn handle_response(&self, response: JSONRPCResponse) {
        let Some(id) = Self::response_id(&response) else {
            tracing::warn!("Received error response without ID");
            return;
        };

        tracing::debug!(
            "Handling response for ID: {}, pending requests: {}",
            id,
            self.inner.pending_requests.len()
        );

        self.dispatch_response(&id, response);
    }

    /// Extract the request ID key from a JSON-RPC response.
    fn response_id(response: &JSONRPCResponse) -> Option<RequestId> {
        match response {
            JSONRPCResponse::Result(result) => Some(result.id.clone()),
            JSONRPCResponse::Error(error) => error.id.clone(),
        }
    }

    /// Route a response to the pending request receiver, if present.
    fn dispatch_response(&self, id: &RequestId, response: JSONRPCResponse) {
        if let Some((_, pending)) = self.inner.pending_requests.remove(id) {
            if pending.response_tx.send(response).is_err() {
                tracing::debug!("Response receiver dropped for request {}", id);
            }
        } else {
            tracing::warn!("Received response for unknown request ID: {}", id);
        }
    }
}

/// Build the method and params of a JSON-RPC request from a typed request.
pub fn request_body<Req>(request: &Req) -> Result<Request>
where
    Req: serde::Serialize + RequestMethod,
{
    let mut params_obj = match serde_json::to_value(request)? {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    params_obj.remove("method");
    Ok(Request {
        method: request.method().to_string(),
        params: request_params(params_obj),
    })
}

/// Trait for types that can provide a method name for requests.
pub trait RequestMethod {
    /// Return the JSON-RPC method name for this request type.
    fn method(&self) -> &'static str;
}

impl RequestMethod for ClientRequest {
    fn method(&self) -> &'static str {
        self.method()
    }
}

impl RequestMethod for ServerRequest {
    fn method(&self) -> &'static str {
        self.method()
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use tokio::{
        io::{DuplexStream, duplex},
        time::timeout,
    };

    use super::*;
    use crate::{
        schema::{ErrorObject, JSONRPCErrorResponse, JSONRPCResult, JSONRPCResultResponse},
        transport::{StreamTransport, Transport},
    };

    #[test]
    fn a_short_deadline_keeps_a_deadline() {
        assert_eq!(timeout_millis(Duration::ZERO), NO_TIMEOUT_MS);
        assert_eq!(timeout_millis(Duration::from_micros(500)), 1);
        assert_eq!(timeout_millis(Duration::from_millis(1500)), 1500);
    }

    #[tokio::test]
    async fn a_deadline_past_the_range_of_an_instant_is_no_deadline() {
        let (handler, _peer) = handler_over(64 * 1024);
        let (_, pending) = handler
            .request_raw_pending(large_request(), Some(Duration::MAX), None)
            .await
            .expect("the request is sent");
        assert!(timeout(Duration::from_millis(10), pending).await.is_err());
    }

    #[test]
    fn test_request_id_display() {
        let string_id = RequestId::String("abc".to_string());
        assert_eq!(format!("{string_id}"), "abc");

        let num_id = RequestId::Number(123);
        assert_eq!(format!("{num_id}"), "123");
    }

    /// Create a request handler over a transport with a buffer of `buffer`
    /// bytes, and return the other end of the transport, which never reads.
    fn handler_over(buffer: usize) -> (RequestHandler, DuplexStream) {
        let (local, peer) = duplex(buffer);
        let stream = Box::new(StreamTransport::new(local))
            .framed()
            .expect("framed transport");
        let (sink, _stream) = stream.split();
        let handler = RequestHandler::new(Some(Arc::new(TokioMutex::new(sink))), "test".into());
        (handler, peer)
    }

    /// A request body larger than the transport buffer of a blocked handler.
    fn large_request() -> Request {
        let mut params = serde_json::Map::new();
        params.insert("data".into(), serde_json::json!("x".repeat(1024)));
        Request {
            method: "test/large".into(),
            params: request_params(params),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_send_ends_at_the_deadline_and_cancels_the_request() {
        let (handler, _peer) = handler_over(16);
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel();

        let result = timeout(
            Duration::from_secs(60),
            handler.request_raw_pending(
                large_request(),
                Some(Duration::from_secs(1)),
                Some(cancel_tx),
            ),
        )
        .await
        .expect("the deadline ends the send");

        assert!(
            matches!(result, Err(Error::Timeout { ref request_id, timeout_ms: 1000 }) if request_id == "test-1"),
            "Expected timeout error, got {:?}",
            result.map(|(id, _)| id)
        );
        assert!(handler.inner.pending_requests.is_empty());
        assert_eq!(
            cancel_rx.try_recv().expect("a cancellation notice"),
            RequestId::String("test-1".into())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_dropped_during_its_send_releases_its_slot_and_cancels() {
        let (handler, _peer) = handler_over(16);
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel();

        let send = handler.request_raw_pending(
            large_request(),
            Some(Duration::from_secs(60)),
            Some(cancel_tx),
        );
        // The caller stops waiting while the send is blocked.
        assert!(timeout(Duration::from_secs(1), send).await.is_err());

        assert!(handler.inner.pending_requests.is_empty());
        assert_eq!(
            cancel_rx.try_recv().expect("a cancellation notice"),
            RequestId::String("test-1".into())
        );
    }

    #[tokio::test]
    async fn a_dropped_pending_request_releases_its_slot_and_cancels() {
        let (handler, _peer) = handler_over(64 * 1024);
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel();
        let (id, pending) = handler
            .request_raw_pending(large_request(), None, Some(cancel_tx))
            .await
            .expect("the request is sent");
        assert_eq!(handler.inner.pending_requests.len(), 1);

        drop(pending);

        assert!(handler.inner.pending_requests.is_empty());
        assert_eq!(cancel_rx.try_recv().expect("a cancellation notice"), id);
    }

    #[tokio::test]
    async fn an_answered_request_sends_no_cancellation() {
        let (handler, _peer) = handler_over(64 * 1024);
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel();
        let (id, pending) = handler
            .request_raw_pending(large_request(), None, Some(cancel_tx))
            .await
            .expect("the request is sent");

        handler
            .handle_response(JSONRPCResponse::Result(JSONRPCResultResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: id.clone(),
                result: JSONRPCResult {
                    _meta: None,
                    other: serde_json::Map::new(),
                },
            }))
            .await;
        pending.await.expect("the response");

        assert!(handler.inner.pending_requests.is_empty());
        assert!(cancel_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_delivered_unpolled_response_sends_no_cancellation_when_dropped() {
        let (handler, _peer) = handler_over(64 * 1024);
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel();
        let (id, pending) = handler
            .request_raw_pending(large_request(), None, Some(cancel_tx))
            .await
            .expect("the request is sent");

        handler
            .handle_response(JSONRPCResponse::Result(JSONRPCResultResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id,
                result: JSONRPCResult {
                    _meta: None,
                    other: serde_json::Map::new(),
                },
            }))
            .await;
        drop(pending);

        assert!(handler.inner.pending_requests.is_empty());
        assert!(cancel_rx.try_recv().is_err());
    }

    #[test]
    fn authorization_error_response_decodes_to_authorization_failed() {
        let response = JSONRPCResponse::Error(JSONRPCErrorResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(RequestId::String("1".to_string())),
            error: ErrorObject {
                code: AUTHORIZATION_FAILED,
                message: "Authorization failed: HTTP 401 Unauthorized".to_string(),
                data: None,
            },
        });

        let error = RequestHandler::decode_response::<serde_json::Value>("tools/call", response)
            .expect_err("authorization error");

        assert!(matches!(error, Error::AuthorizationFailed(_)));
        assert!(
            error
                .to_string()
                .contains("Authorization failed: HTTP 401 Unauthorized")
        );
    }
}
