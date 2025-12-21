use std::{
    collections::HashMap,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use dashmap::DashMap;
use futures::{SinkExt, stream::SplitSink};
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, oneshot};

use crate::{
    error::{Error, Result},
    schema::{
        INVALID_PARAMS, JSONRPC_VERSION, JSONRPCError, JSONRPCMessage, JSONRPCNotification,
        JSONRPCRequest, JSONRPCResponse, METHOD_NOT_FOUND, Notification, NotificationParams,
        Request, RequestId, RequestParams,
    },
    transport::TransportStream,
};

/// Type for handling either a response or error from JSON-RPC
#[derive(Debug)]
pub enum ResponseOrError {
    /// Successful JSON-RPC response payload.
    Response(JSONRPCResponse),
    /// JSON-RPC error payload.
    Error(JSONRPCError),
}

/// Transport sink type used by the request handler.
pub type TransportSink = Arc<Mutex<SplitSink<Box<dyn TransportStream>, JSONRPCMessage>>>;

/// Common request/response handling functionality shared between Client and ServerCtx
#[derive(Clone)]
pub struct RequestHandler {
    /// Transport sink for sending messages
    transport_tx: Option<TransportSink>,
    /// Pending requests waiting for responses
    pending_requests: Arc<DashMap<String, oneshot::Sender<ResponseOrError>>>,
    /// Next request ID counter
    next_request_id: Arc<AtomicU64>,
    /// Prefix for request IDs (e.g., "req" for client, "srv-req" for server)
    id_prefix: String,
}

impl RequestHandler {
    /// Create a new RequestHandler
    pub fn new(transport_tx: Option<TransportSink>, id_prefix: String) -> Self {
        Self {
            transport_tx,
            pending_requests: Arc::new(DashMap::new()),
            next_request_id: Arc::new(AtomicU64::new(1)),
            id_prefix,
        }
    }

    /// Set the transport sink (used when connecting)
    pub fn set_transport(&mut self, transport_tx: TransportSink) {
        self.transport_tx = Some(transport_tx);
    }

    /// Send a request and wait for response
    pub async fn request<Req, Res>(&self, request: Req) -> Result<Res>
    where
        Req: serde::Serialize + RequestMethod,
        Res: DeserializeOwned,
    {
        let id = self.next_request_id().await;
        let (tx, rx) = oneshot::channel();

        // Store the response channel
        self.pending_requests.insert(id.clone(), tx);
        tracing::debug!(
            "Stored pending request with ID: {}, total pending: {}",
            id,
            self.pending_requests.len()
        );

        // Create the JSON-RPC request
        let jsonrpc_request = Self::build_request(&id, &request)?;

        tracing::info!(
            "Sending request with ID: {} method: {}",
            id,
            request.method()
        );
        self.send_message(JSONRPCMessage::Request(jsonrpc_request))
            .await?;

        // Wait for response
        Self::process_response(rx.await, &id, request.method(), &self.pending_requests)
    }

    /// Send a message through the transport
    pub async fn send_message(&self, message: JSONRPCMessage) -> Result<()> {
        if let Some(transport_tx) = &self.transport_tx {
            let mut tx = transport_tx.lock().await;
            tx.send(message).await?;
            Ok(())
        } else {
            Err(Error::Transport("Not connected".to_string()))
        }
    }

    /// Send a notification
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

    /// Generate the next request ID
    async fn next_request_id(&self) -> String {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        format!("{}-{}", self.id_prefix, id)
    }

    /// Build a JSON-RPC request message from a typed request.
    fn build_request<Req>(id: &str, request: &Req) -> Result<JSONRPCRequest>
    where
        Req: serde::Serialize + RequestMethod,
    {
        Ok(JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::String(id.to_string()),
            request: Request {
                method: request.method().to_string(),
                params: Some(RequestParams {
                    _meta: None,
                    other: serde_json::to_value(request)?
                        .as_object()
                        .unwrap_or(&serde_json::Map::new())
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                }),
            },
        })
    }

    /// Decode a response payload into the expected result type.
    fn decode_response<Res>(method: &str, response: ResponseOrError) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        match response {
            ResponseOrError::Response(response) => {
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
            ResponseOrError::Error(error) => match error.error.code {
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

    /// Wait for a response channel and convert it into the expected result.
    fn process_response<Res>(
        response: StdResult<ResponseOrError, oneshot::error::RecvError>,
        id: &str,
        method: &str,
        pending_requests: &DashMap<String, oneshot::Sender<ResponseOrError>>,
    ) -> Result<Res>
    where
        Res: DeserializeOwned,
    {
        match response {
            Ok(response_or_error) => Self::decode_response(method, response_or_error),
            Err(e) => {
                tracing::error!("Response channel closed for request {}: {}", id, e);
                pending_requests.remove(id);
                Err(Error::Protocol("Response channel closed".to_string()))
            }
        }
    }

    /// Handle a response from the remote side
    pub async fn handle_response(&self, response: JSONRPCResponse) {
        let RequestId::String(id) = &response.id else {
            return;
        };

        let id = id.clone();
        tracing::debug!(
            "Handling response for ID: {}, pending requests: {:?}",
            id,
            self.pending_requests.len()
        );

        if let Some((_, tx)) = self.pending_requests.remove(&id) {
            if tx.send(ResponseOrError::Response(response)).is_err() {
                tracing::debug!("Response receiver dropped for request {}", id);
            }
        } else {
            tracing::warn!("Received response for unknown request ID: {}", id);
        }
    }

    /// Handle an error response from the remote side
    pub async fn handle_error(&self, error: JSONRPCError) {
        let RequestId::String(id) = &error.id else {
            return;
        };

        let id = id.clone();
        if let Some((_, tx)) = self.pending_requests.remove(&id) {
            if tx.send(ResponseOrError::Error(error)).is_err() {
                tracing::debug!("Error receiver dropped for request {}", id);
            }
        } else {
            tracing::warn!("Received error for unknown request ID: {}", id);
        }
    }
}

/// Trait for types that can provide a method name for requests
pub trait RequestMethod {
    /// Return the JSON-RPC method name for this request type.
    fn method(&self) -> &'static str;
}
