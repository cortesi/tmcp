//! Request/response routing and tracking.
//!
//! This module provides the [`RequestHandler`] which correlates outgoing requests
//! with incoming responses, handles timeouts, and supports cancellation.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    result::Result as StdResult,
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
    sync::{Mutex as TokioMutex, oneshot},
    time::{error::Elapsed, timeout},
};

/// Type alias for the nested Result type from timeout + channel receive.
type TimeoutResult<T> = StdResult<StdResult<T, oneshot::error::RecvError>, Elapsed>;

use crate::{
    error::{Error, Result},
    schema::{
        AUTHORIZATION_FAILED, INVALID_PARAMS, JSONRPC_VERSION, JSONRPCMessage, JSONRPCNotification,
        JSONRPCRequest, JSONRPCResponse, METHOD_NOT_FOUND, Notification, NotificationParams,
        Request, RequestId, RequestMeta, RequestParams,
    },
    transport::TransportStream,
};

/// Default request timeout in milliseconds (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Transport sink type used by the request handler.
pub type TransportSink = Arc<TokioMutex<SplitSink<Box<dyn TransportStream>, JSONRPCMessage>>>;

/// Metadata for a pending request.
struct PendingRequest {
    /// Channel to send the response back to the caller.
    response_tx: oneshot::Sender<JSONRPCResponse>,
}

/// A typed response future for an outbound JSON-RPC request.
pub struct Pending<Res> {
    /// Future waiting for and decoding the response.
    inner: Pin<Box<dyn Future<Output = Result<Res>> + Send + 'static>>,
    /// Cleanup state for removing the request slot if the future is dropped.
    cleanup: PendingCleanup,
}

/// State needed to clean up a dropped pending request.
struct PendingCleanup {
    /// Shared handler that owns pending request storage.
    handler: RequestHandler,
    /// Request id to remove on drop.
    id: RequestId,
}

impl<Res> Future for Pending<Res> {
    type Output = Result<Res>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl<Res> Drop for Pending<Res> {
    fn drop(&mut self) {
        self.cleanup
            .handler
            .inner
            .pending_requests
            .remove(&self.cleanup.id);
    }
}

/// Common request/response handling functionality shared between Client and ServerCtx.
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

    /// Set the request timeout in milliseconds.
    pub fn with_timeout(self, timeout_ms: u64) -> Self {
        self.inner.timeout_ms.store(timeout_ms, Ordering::Relaxed);
        self
    }

    /// Shut down the current transport and wake any pending requests immediately.
    pub fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        if let Ok(mut lock) = self.inner.transport_tx.lock() {
            *lock = None;
        }
        self.inner.pending_requests.clear();
    }

    /// Send a request and wait for response with timeout and cancellation support.
    pub async fn request<Req, Res>(&self, request: Req) -> Result<Res>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        let (_, pending) = self.request_pending(request).await?;
        pending.await
    }

    /// Send a request and return its id plus a typed response future.
    pub async fn request_pending<Req, Res>(&self, request: Req) -> Result<(RequestId, Pending<Res>)>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned + Send + 'static,
    {
        let id = self.next_request_id();
        let method = request.method().to_string();

        // Store and send the request
        let response_rx = self.store_and_send_request(id.clone(), &request).await?;
        let handler = self.clone();
        let pending_id = id.clone();
        let cleanup = PendingCleanup {
            handler: self.clone(),
            id: id.clone(),
        };
        let inner = Box::pin(async move {
            handler
                .await_response(pending_id, &method, response_rx)
                .await
        });

        Ok((id, Pending { inner, cleanup }))
    }

    /// Store a pending request and send it over the transport.
    async fn store_and_send_request<Req>(
        &self,
        id: RequestId,
        request: &Req,
    ) -> Result<oneshot::Receiver<JSONRPCResponse>>
    where
        Req: serde::Serialize + RequestMethod,
    {
        let (response_tx, response_rx) = oneshot::channel();

        self.inner
            .pending_requests
            .insert(id.clone(), PendingRequest { response_tx });

        tracing::debug!(
            "Stored pending request with ID: {}, total pending: {}",
            id,
            self.inner.pending_requests.len()
        );

        let jsonrpc_request = Self::build_request(id.clone(), request)?;
        tracing::info!(
            "Sending request with ID: {} method: {}",
            id,
            request.method()
        );

        if let Err(e) = self
            .send_message(JSONRPCMessage::Request(jsonrpc_request))
            .await
        {
            self.inner.pending_requests.remove(&id);
            return Err(e);
        }

        Ok(response_rx)
    }

    /// Wait for a response with timeout and cancellation support.
    async fn await_response<Res>(
        &self,
        id: RequestId,
        method: &str,
        response_rx: oneshot::Receiver<JSONRPCResponse>,
    ) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        let timeout_ms = self.inner.timeout_ms.load(Ordering::Relaxed);
        let timeout_duration = Duration::from_millis(timeout_ms);
        let result = timeout(timeout_duration, response_rx).await;
        self.process_response_result(&id, method, result)
    }

    /// Process the result of waiting for a response.
    fn process_response_result<Res>(
        &self,
        id: &RequestId,
        method: &str,
        result: TimeoutResult<JSONRPCResponse>,
    ) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        match result {
            Ok(Ok(response)) => Self::decode_response(method, response),
            Ok(Err(_recv_error)) => {
                self.inner.pending_requests.remove(id);
                if self.inner.shutting_down.load(Ordering::Acquire) {
                    Err(Error::TransportDisconnected)
                } else {
                    Err(Error::Protocol(
                        "Response channel closed unexpectedly".to_string(),
                    ))
                }
            }
            Err(_timeout) => {
                self.inner.pending_requests.remove(id);
                let timeout_ms = self.inner.timeout_ms.load(Ordering::Relaxed);
                tracing::warn!("Request {} timed out after {}ms", id, timeout_ms);
                Err(Error::Timeout {
                    request_id: id.to_string(),
                    timeout_ms,
                })
            }
        }
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
            other: if let Some(obj) = v.as_object() {
                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            } else {
                HashMap::new()
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

    /// Build a JSON-RPC request message from a typed request.
    fn build_request<Req>(id: RequestId, request: &Req) -> Result<JSONRPCRequest>
    where
        Req: serde::Serialize + RequestMethod,
    {
        let mut params_obj = serde_json::to_value(request)?
            .as_object()
            .cloned()
            .unwrap_or_default();
        params_obj.remove("method");
        let meta = params_obj
            .remove("_meta")
            .and_then(|value| serde_json::from_value::<RequestMeta>(value).ok());
        let params = if params_obj.is_empty() && meta.is_none() {
            None
        } else {
            Some(RequestParams {
                _meta: meta,
                other: params_obj.into_iter().collect(),
            })
        };
        Ok(JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            request: Request {
                method: request.method().to_string(),
                params,
            },
        })
    }

    /// Decode a response payload into the expected result type.
    fn decode_response<Res>(method: &str, response: JSONRPCResponse) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        match response {
            JSONRPCResponse::Result(response) => {
                let mut result_value = serde_json::Map::new();

                if let Some(meta) = response.result._meta {
                    result_value.insert("_meta".to_string(), serde_json::to_value(meta)?);
                }

                for (key, value) in response.result.other {
                    result_value.insert(key, value);
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

/// Trait for types that can provide a method name for requests.
pub trait RequestMethod {
    /// Return the JSON-RPC method name for this request type.
    fn method(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use futures::future::pending;

    use super::*;
    use crate::schema::{ErrorObject, JSONRPCErrorResponse};

    #[test]
    fn test_request_id_to_key_string() {
        let id = RequestId::String("test-123".to_string());
        assert_eq!(id.to_key(), "test-123");
    }

    #[test]
    fn test_request_id_to_key_number() {
        let id = RequestId::Number(42);
        assert_eq!(id.to_key(), "__num__42");
    }

    #[test]
    fn test_request_id_display() {
        let string_id = RequestId::String("abc".to_string());
        assert_eq!(format!("{string_id}"), "abc");

        let num_id = RequestId::Number(123);
        assert_eq!(format!("{num_id}"), "123");
    }

    #[test]
    fn test_pending_drop_cleans_request_slot() {
        let handler = RequestHandler::new(None, "test".to_string());
        let id = handler.next_request_id();
        let (response_tx, _response_rx) = oneshot::channel();

        handler
            .inner
            .pending_requests
            .insert(id.clone(), PendingRequest { response_tx });

        let pending: Pending<serde_json::Value> = Pending {
            inner: Box::pin(pending()),
            cleanup: PendingCleanup {
                handler: handler.clone(),
                id,
            },
        };

        assert_eq!(handler.inner.pending_requests.len(), 1);
        drop(pending);
        assert!(handler.inner.pending_requests.is_empty());
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
