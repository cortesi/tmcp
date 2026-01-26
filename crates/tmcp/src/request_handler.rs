//! Request/response routing and tracking.
//!
//! This module provides the [`RequestHandler`] which correlates outgoing requests
//! with incoming responses, handles timeouts, and supports cancellation.

use std::{
    collections::HashMap,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use dashmap::DashMap;
use futures::{SinkExt, stream::SplitSink};
use serde::de::DeserializeOwned;
use tokio::{
    sync::{Mutex, oneshot},
    time::{error::Elapsed, timeout},
};
use tokio_util::sync::CancellationToken;

/// Type alias for the nested Result type from timeout + channel receive.
type TimeoutResult<T> = StdResult<StdResult<T, oneshot::error::RecvError>, Elapsed>;

use crate::{
    error::{Error, Result},
    schema::{
        INVALID_PARAMS, JSONRPC_VERSION, JSONRPCMessage, JSONRPCNotification, JSONRPCRequest,
        JSONRPCResponse, METHOD_NOT_FOUND, Notification, NotificationParams, Request, RequestId,
        RequestMeta, RequestParams,
    },
    transport::TransportStream,
};

/// Default request timeout in milliseconds (30 seconds).
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Transport sink type used by the request handler.
pub type TransportSink = Arc<Mutex<SplitSink<Box<dyn TransportStream>, JSONRPCMessage>>>;

/// Metadata for a pending request.
struct PendingRequest {
    /// Channel to send the response back to the caller.
    response_tx: oneshot::Sender<JSONRPCResponse>,
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
    /// Transport sink for sending messages.
    transport_tx: Option<TransportSink>,
    /// Pending requests waiting for responses, keyed by request ID.
    pending_requests: Arc<DashMap<RequestId, PendingRequest>>,
    /// Next request ID counter.
    next_request_id: Arc<AtomicU64>,
    /// Prefix for request IDs (e.g., "req" for client, "srv-req" for server).
    id_prefix: String,
    /// Request timeout in milliseconds.
    timeout_ms: u64,
    /// Global cancellation token for all requests (used during shutdown).
    global_cancel: CancellationToken,
}

impl RequestHandler {
    /// Create a new RequestHandler with default timeout.
    pub fn new(transport_tx: Option<TransportSink>, id_prefix: String) -> Self {
        Self {
            transport_tx,
            pending_requests: Arc::new(DashMap::new()),
            next_request_id: Arc::new(AtomicU64::new(1)),
            id_prefix,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            global_cancel: CancellationToken::new(),
        }
    }

    /// Set the transport sink (used when connecting).
    pub fn set_transport(&mut self, transport_tx: TransportSink) {
        self.transport_tx = Some(transport_tx);
    }

    /// Set the request timeout in milliseconds.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Send a request and wait for response with timeout and cancellation support.
    pub async fn request<Req, Res>(&self, request: Req) -> Result<Res>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned,
    {
        let id = self.next_request_id();

        // Store and send the request
        let response_rx = self.store_and_send_request(id.clone(), &request).await?;

        // Wait for response with timeout and cancellation
        self.await_response(id, request.method(), response_rx).await
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

        self.pending_requests
            .insert(id.clone(), PendingRequest { response_tx });

        tracing::debug!(
            "Stored pending request with ID: {}, total pending: {}",
            id,
            self.pending_requests.len()
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
            self.pending_requests.remove(&id);
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
        let timeout_duration = Duration::from_millis(self.timeout_ms);

        tokio::select! {
            biased;

            () = self.global_cancel.cancelled() => {
                self.pending_requests.remove(&id);
                Err(Error::Cancelled { request_id: id.to_string() })
            }

            result = timeout(timeout_duration, response_rx) => {
                self.process_response_result(&id, method, result)
            }
        }
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
                self.pending_requests.remove(id);
                Err(Error::Protocol(
                    "Response channel closed unexpectedly".to_string(),
                ))
            }
            Err(_timeout) => {
                self.pending_requests.remove(id);
                tracing::warn!("Request {} timed out after {}ms", id, self.timeout_ms);
                Err(Error::Timeout {
                    request_id: id.to_string(),
                    timeout_ms: self.timeout_ms,
                })
            }
        }
    }

    /// Send a message through the transport.
    pub async fn send_message(&self, message: JSONRPCMessage) -> Result<()> {
        if let Some(transport_tx) = &self.transport_tx {
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
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        RequestId::String(format!("{}-{}", self.id_prefix, id))
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
            self.pending_requests.len()
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
        if let Some((_, pending)) = self.pending_requests.remove(id) {
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
    use super::*;

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
}
