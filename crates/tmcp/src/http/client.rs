//! HTTP client transport: streamable HTTP with SSE support.

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use eventsource_stream::Eventsource;
use futures::{Sink, Stream, StreamExt, channel::mpsc};
use reqwest::Client as HttpClient;
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::{
    auth::OAuth2Client,
    error::{Error, Result},
    schema::{
        AUTHORIZATION_FAILED, ErrorObject, INTERNAL_ERROR, JSONRPC_VERSION, JSONRPCErrorResponse,
        JSONRPCMessage, JSONRPCResponse, LATEST_PROTOCOL_VERSION,
    },
    transport::{IncomingMessage, Transport, TransportStream},
};

/// Default HTTP client timeout for transport requests.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Tracks whether SSE streaming is active and supported by the server.
#[derive(Debug)]
struct HttpSseState {
    /// True while the SSE task is running.
    running: AtomicBool,
    /// True if the server supports SSE for streamable HTTP.
    supported: AtomicBool,
}

impl HttpSseState {
    /// Create a new SSE state tracker.
    fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            supported: AtomicBool::new(true),
        }
    }
}

/// HTTP client transport
pub struct HttpClientTransport {
    /// Endpoint URL for HTTP transport.
    endpoint: String,
    /// HTTP client used to send requests.
    client: HttpClient,
    /// Session identifier returned by the server.
    session_id: Arc<Mutex<Option<String>>>,
    /// Last observed SSE event id.
    last_event_id: Arc<Mutex<Option<String>>>,
    /// Headers attached to every HTTP request.
    static_headers: HeaderMap,
    /// Sender half for outbound JSON-RPC messages.
    sender: Option<mpsc::UnboundedSender<JSONRPCMessage>>,
    /// Receiver half for inbound JSON-RPC messages.
    receiver: Option<mpsc::UnboundedReceiver<JSONRPCMessage>>,
    /// SSE connection state for streamable HTTP.
    sse_state: Arc<HttpSseState>,
    /// Cancellation token for SSE shutdown.
    sse_shutdown: CancellationToken,
    /// OAuth client for attaching bearer tokens.
    oauth_client: Option<Arc<OAuth2Client>>,
}

/// Stream wrapper for HTTP transport
struct HttpTransportStream {
    /// Sender for outgoing JSON-RPC messages.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// Receiver for incoming JSON-RPC messages.
    receiver: mpsc::UnboundedReceiver<JSONRPCMessage>,
    /// Join handle for the background HTTP task.
    _http_task: JoinHandle<()>,
    /// Cancellation token for SSE shutdown.
    sse_shutdown: CancellationToken,
}

impl Drop for HttpTransportStream {
    fn drop(&mut self) {
        // Cancel the HTTP task when the stream is dropped
        self._http_task.abort();
        self.sse_shutdown.cancel();
    }
}

/// Capture a session id from response headers for initialize requests.
async fn update_session_id(
    is_initialize: bool,
    headers: &HeaderMap,
    session_id: &Arc<Mutex<Option<String>>>,
) -> Option<String> {
    if !is_initialize {
        return None;
    }

    let sid = headers.get("Mcp-Session-Id")?;

    let Ok(sid_str) = sid.to_str() else {
        return None;
    };

    let mut guard = session_id.lock().await;
    let updated = sid_str.to_string();
    *guard = Some(updated.clone());
    debug!("Got session ID: {}", sid_str);
    Some(updated)
}

/// Return true if the message expects a JSON-RPC response body.
fn expects_response(msg: &JSONRPCMessage) -> bool {
    matches!(msg, JSONRPCMessage::Request(_))
}

/// Log non-success HTTP responses and return whether processing should continue.
fn validate_status(status: reqwest::StatusCode) -> bool {
    if status.is_success() {
        true
    } else {
        error!("HTTP request failed with status: {}", status);
        false
    }
}

/// Parse a JSON-RPC response and forward it over the channel.
async fn forward_response(
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
) {
    match response.json::<JSONRPCMessage>().await {
        Ok(response_msg) => {
            debug!("HTTP client received response: {:?}", response_msg);
            if let Err(e) = sender.unbounded_send(response_msg) {
                error!("Failed to forward response: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to parse response: {}", e);
        }
    }
}

/// Return true if a response is an SSE stream.
fn response_is_sse(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// Parse an SSE response stream and forward JSON-RPC messages.
async fn forward_sse_response(
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
    last_event_id: &Arc<Mutex<Option<String>>>,
) {
    let stream = response.bytes_stream().eventsource();
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                let event_id = event.id;
                if !event_id.is_empty() {
                    let mut guard = last_event_id.lock().await;
                    *guard = Some(event_id);
                }
                let data = event.data;
                if let Ok(msg) = serde_json::from_str::<JSONRPCMessage>(&data)
                    && sender.unbounded_send(msg).is_err()
                {
                    break;
                }
            }
            Err(e) => {
                error!("SSE response error: {:?}", e);
                break;
            }
        }
    }
}

/// Handle an HTTP response for a single outbound JSON-RPC message.
async fn handle_http_response(
    msg: &JSONRPCMessage,
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
    last_event_id: &Arc<Mutex<Option<String>>>,
) {
    if !validate_status(response.status()) {
        forward_status_error(msg, response.status(), sender);
        return;
    }

    if !expects_response(msg) {
        return;
    }

    if matches!(
        response.status(),
        StatusCode::ACCEPTED | StatusCode::NO_CONTENT
    ) {
        return;
    }

    if response_is_sse(&response) {
        forward_sse_response(response, sender, last_event_id).await;
    } else {
        forward_response(response, sender).await;
    }
}

/// Forward an HTTP status failure to a waiting JSON-RPC request, if any.
fn forward_status_error(
    msg: &JSONRPCMessage,
    status: reqwest::StatusCode,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
) {
    let code = if status == StatusCode::UNAUTHORIZED {
        AUTHORIZATION_FAILED
    } else {
        INTERNAL_ERROR
    };
    let message = if status == StatusCode::UNAUTHORIZED {
        "Authorization failed: HTTP 401 Unauthorized".to_string()
    } else {
        format!("HTTP request failed with status: {status}")
    };
    forward_request_error(msg, code, message, sender);
}

/// Forward a synthetic JSON-RPC error to a waiting request, if any.
fn forward_request_error(
    msg: &JSONRPCMessage,
    code: i32,
    message: String,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
) {
    let JSONRPCMessage::Request(request) = msg else {
        return;
    };
    let response = JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(request.id.clone()),
        error: ErrorObject {
            code,
            message,
            data: None,
        },
    }));
    if sender.unbounded_send(response).is_err() {
        error!("Failed to forward synthetic HTTP error response");
    }
}

impl HttpClientTransport {
    /// Create a new HTTP client transport for the provided endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        let mut client = HttpClient::builder().timeout(DEFAULT_TIMEOUT);
        if endpoint.starts_with("http://127.0.0.1:")
            || endpoint.starts_with("http://localhost:")
            || endpoint.starts_with("http://[::1]:")
        {
            client = client.no_proxy();
        }
        Self {
            endpoint,
            client: client.build().expect("Failed to create HTTP client"),
            session_id: Arc::new(Mutex::new(None)),
            last_event_id: Arc::new(Mutex::new(None)),
            static_headers: HeaderMap::new(),
            sender: None,
            receiver: None,
            sse_state: Arc::new(HttpSseState::new()),
            sse_shutdown: CancellationToken::new(),
            oauth_client: None,
        }
    }

    /// Attach an OAuth client used to fetch bearer tokens for requests.
    pub fn with_oauth(mut self, oauth_client: Arc<OAuth2Client>) -> Self {
        self.oauth_client = Some(oauth_client);
        self
    }

    /// Attach static headers to every POST and SSE request.
    pub fn with_static_headers(mut self, headers: HeaderMap) -> Self {
        self.static_headers = sensitive_header_map(headers);
        self
    }

    /// Connect to SSE stream for receiving server messages
    async fn connect_sse(context: SseConnectContext) -> Result<SseOutcome> {
        let Some(session_id_value) = context.session_id.lock().await.clone() else {
            return Ok(SseOutcome::NoSession);
        };

        let mut headers = context.static_headers;
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            "MCP-Protocol-Version",
            HeaderValue::from_static(LATEST_PROTOCOL_VERSION),
        );

        headers.insert(
            "Mcp-Session-Id",
            HeaderValue::from_str(&session_id_value)
                .map_err(|_| Error::Transport("Invalid session ID".into()))?,
        );

        if let Some(last_event_id_value) = context.last_event_id.lock().await.clone() {
            headers.insert(
                "Last-Event-ID",
                HeaderValue::from_str(&last_event_id_value)
                    .map_err(|_| Error::Transport("Invalid Last-Event-ID".into()))?,
            );
        }

        // Add OAuth authorization header if available
        if let Some(oauth_client) = &context.oauth_client {
            let token = oauth_client.get_valid_token().await?;
            headers.insert(header::AUTHORIZATION, bearer_header(&token)?);
        }

        let response = context
            .client
            .get(&context.endpoint)
            .headers(headers)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("Failed to connect SSE: {e}")))?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            // Server doesn't support SSE endpoint
            return Ok(SseOutcome::NotSupported);
        }

        if !response.status().is_success() {
            return Err(Error::Transport(format!(
                "SSE connection failed with status: {}",
                response.status()
            )));
        }

        let stream = response.bytes_stream().eventsource();
        futures::pin_mut!(stream);
        loop {
            tokio::select! {
                _ = context.shutdown.cancelled() => break,
                event = stream.next() => {
                    match event {
                        Some(Ok(event)) => {
                            let event_id = event.id;
                            if !event_id.is_empty() {
                                let mut guard = context.last_event_id.lock().await;
                                *guard = Some(event_id);
                            }
                            let data = event.data;
                            if let Ok(msg) = serde_json::from_str::<JSONRPCMessage>(&data)
                                && context.sender.unbounded_send(msg).is_err()
                            {
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            error!("SSE error: {:?}", e);
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(SseOutcome::Closed)
    }
}

/// Context required for one SSE connection attempt.
struct SseConnectContext {
    /// HTTP client used for SSE connection.
    client: HttpClient,
    /// Endpoint URL for the SSE stream.
    endpoint: String,
    /// Session id for streamable HTTP.
    session_id: Arc<Mutex<Option<String>>>,
    /// Most recent SSE event id observed.
    last_event_id: Arc<Mutex<Option<String>>>,
    /// Headers attached to the SSE request.
    static_headers: HeaderMap,
    /// Sender for forwarding JSON-RPC messages.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// OAuth client for auth headers, if configured.
    oauth_client: Option<Arc<OAuth2Client>>,
    /// Cancellation token to stop SSE processing.
    shutdown: CancellationToken,
}

/// Context required to start or restart an SSE stream.
struct SseStartContext {
    /// HTTP client used for SSE connection.
    client: HttpClient,
    /// Endpoint URL for the SSE stream.
    endpoint: String,
    /// Session id for streamable HTTP.
    session_id: Arc<Mutex<Option<String>>>,
    /// Most recent SSE event id observed.
    last_event_id: Arc<Mutex<Option<String>>>,
    /// Headers attached to every SSE request.
    static_headers: HeaderMap,
    /// Sender for forwarding JSON-RPC messages.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// OAuth client for auth headers, if configured.
    oauth_client: Option<Arc<OAuth2Client>>,
    /// Shared SSE state tracking.
    sse_state: Arc<HttpSseState>,
    /// Cancellation token to stop SSE processing.
    shutdown: CancellationToken,
}

/// Start SSE processing if not already running.
fn maybe_start_sse(context: SseStartContext) {
    if !context.sse_state.supported.load(Ordering::SeqCst) {
        return;
    }

    if context.sse_state.running.swap(true, Ordering::SeqCst) {
        return;
    }

    tokio::spawn(async move {
        let outcome = HttpClientTransport::connect_sse(SseConnectContext {
            client: context.client,
            endpoint: context.endpoint,
            session_id: context.session_id,
            last_event_id: context.last_event_id,
            static_headers: context.static_headers,
            sender: context.sender,
            oauth_client: context.oauth_client,
            shutdown: context.shutdown,
        })
        .await;

        match outcome {
            Ok(SseOutcome::NotSupported) => {
                context.sse_state.supported.store(false, Ordering::SeqCst);
            }
            Ok(_) => {}
            Err(err) => {
                debug!("SSE connection failed (server may not support it): {}", err);
            }
        }

        context.sse_state.running.store(false, Ordering::SeqCst);
    });
}

/// Outcome of an SSE connection attempt.
#[derive(Debug, Clone, Copy)]
enum SseOutcome {
    /// Session id not yet available.
    NoSession,
    /// Server does not support SSE at the endpoint.
    NotSupported,
    /// Stream ended or was cancelled.
    Closed,
}

#[async_trait]
impl Transport for HttpClientTransport {
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to HTTP endpoint: {}", self.endpoint);

        // Create channels for bidirectional communication
        let (tx, rx) = mpsc::unbounded();
        self.sender = Some(tx);
        self.receiver = Some(rx);

        Ok(())
    }

    fn framed(mut self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        let sender = self.sender.take().ok_or(Error::TransportDisconnected)?;
        let receiver = self.receiver.take().ok_or(Error::TransportDisconnected)?;

        // Create a task to handle sending messages via HTTP
        let endpoint = self.endpoint.clone();
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let last_event_id = self.last_event_id.clone();
        let static_headers = self.static_headers.clone();
        let oauth_client = self.oauth_client.clone();
        let sse_state = self.sse_state.clone();
        let sse_shutdown = self.sse_shutdown.clone();

        let (http_tx, mut http_rx) = mpsc::unbounded::<JSONRPCMessage>();
        let sender_clone = sender;

        let http_task = tokio::spawn(async move {
            while let Some(msg) = http_rx.next().await {
                debug!("HTTP client sending message: {:?}", msg);

                // Check if this is an initialize request to capture session ID
                let is_initialize = matches!(&msg, JSONRPCMessage::Request(req) if req.request.method == "initialize");

                let request = match outbound_post_request(
                    &static_headers,
                    &session_id,
                    oauth_client.as_ref(),
                )
                .await
                {
                    Ok(request) => request,
                    Err(error) => {
                        error!("Failed to prepare HTTP request headers: {}", error);
                        let code = if matches!(error, Error::AuthorizationFailed(_)) {
                            AUTHORIZATION_FAILED
                        } else {
                            INTERNAL_ERROR
                        };
                        forward_request_error(&msg, code, error.to_string(), &sender_clone);
                        continue;
                    }
                };
                let mut response_result =
                    send_http_message(&client, &endpoint, request.headers, &msg).await;

                if matches!(&response_result, Ok(response) if response.status() == StatusCode::UNAUTHORIZED)
                    && let (Some(oauth), Some(sent_token)) =
                        (oauth_client.as_ref(), request.access_token.as_deref())
                {
                    match oauth.refresh_access_token_if_current(sent_token).await {
                        Ok(_) => {
                            match outbound_post_request(&static_headers, &session_id, Some(oauth))
                                .await
                            {
                                Ok(request) => {
                                    response_result = send_http_message(
                                        &client,
                                        &endpoint,
                                        request.headers,
                                        &msg,
                                    )
                                    .await;
                                }
                                Err(error) => {
                                    error!(
                                        "Failed to prepare retried HTTP request headers: {}",
                                        error
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            error!(
                                "OAuth token refresh failed after HTTP 401; re-authentication required: {}",
                                error
                            );
                        }
                    }
                }

                match response_result {
                    Ok(response) => {
                        debug!("HTTP response status: {}", response.status());
                        update_session_id(is_initialize, response.headers(), &session_id).await;
                        handle_http_response(&msg, response, &sender_clone, &last_event_id).await;
                        maybe_start_sse(SseStartContext {
                            client: client.clone(),
                            endpoint: endpoint.clone(),
                            session_id: session_id.clone(),
                            last_event_id: last_event_id.clone(),
                            static_headers: static_headers.clone(),
                            sender: sender_clone.clone(),
                            oauth_client: oauth_client.clone(),
                            sse_state: sse_state.clone(),
                            shutdown: sse_shutdown.clone(),
                        });
                    }
                    Err(e) => {
                        error!("Failed to send HTTP request to {}: {:?}", endpoint, e);
                    }
                }
            }
        });

        Ok(Box::new(HttpTransportStream {
            sender: http_tx,
            receiver,
            _http_task: http_task,
            sse_shutdown: self.sse_shutdown.clone(),
        }))
    }

    fn remote_addr(&self) -> String {
        self.endpoint.clone()
    }
}

/// HTTP POST request data for one outbound JSON-RPC message.
struct OutboundPostRequest {
    /// Headers attached to the request.
    headers: HeaderMap,
    /// Access token attached to the request, if OAuth is configured.
    access_token: Option<String>,
}

/// Builds HTTP POST request data for one outbound JSON-RPC message.
async fn outbound_post_request(
    static_headers: &HeaderMap,
    session_id: &Arc<Mutex<Option<String>>>,
    oauth_client: Option<&Arc<OAuth2Client>>,
) -> Result<OutboundPostRequest> {
    let mut headers = static_headers.clone();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(
        "MCP-Protocol-Version",
        HeaderValue::from_static(LATEST_PROTOCOL_VERSION),
    );

    if let Some(ref sid) = *session_id.lock().await {
        headers.insert(
            "Mcp-Session-Id",
            HeaderValue::from_str(sid)
                .map_err(|_| Error::Transport("Invalid session ID".into()))?,
        );
    }

    let mut access_token = None;
    if let Some(oauth_client) = oauth_client {
        let token = oauth_client.get_valid_token().await?;
        headers.insert(header::AUTHORIZATION, bearer_header(&token)?);
        access_token = Some(token);
    }

    Ok(OutboundPostRequest {
        headers,
        access_token,
    })
}

/// Builds an authorization header without exposing token material through logs.
fn bearer_header(token: &str) -> Result<HeaderValue> {
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Error::Transport("Invalid authorization token".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

/// Marks caller-supplied static headers as sensitive before attaching them.
fn sensitive_header_map(mut headers: HeaderMap) -> HeaderMap {
    for value in headers.values_mut() {
        value.set_sensitive(true);
    }
    headers
}

/// Sends one outbound JSON-RPC message over HTTP.
async fn send_http_message(
    client: &HttpClient,
    endpoint: &str,
    headers: HeaderMap,
    msg: &JSONRPCMessage,
) -> reqwest::Result<reqwest::Response> {
    client
        .post(endpoint)
        .headers(headers)
        .json(msg)
        .send()
        .await
}

impl Stream for HttpTransportStream {
    type Item = Result<IncomingMessage>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_next_unpin(cx) {
            Poll::Ready(Some(message)) => Poll::Ready(Some(Ok(IncomingMessage {
                message,
                extensions: http::Extensions::new(),
            }))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Sink<JSONRPCMessage> for HttpTransportStream {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: JSONRPCMessage) -> Result<()> {
        self.sender
            .unbounded_send(item)
            .map_err(|_| Error::ConnectionClosed)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl TransportStream for HttpTransportStream {}

#[cfg(test)]
mod tests {
    use futures::SinkExt;

    use super::*;
    use crate::schema::{JSONRPCNotification, JSONRPCRequest, Notification, Request, RequestId};

    #[tokio::test]
    async fn test_http_client_transport_creation() {
        let transport = HttpClientTransport::new("http://localhost:8080");
        assert_eq!(transport.endpoint, "http://localhost:8080");
    }

    #[test]
    fn test_http_client_transport_static_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Verber-Auth", HeaderValue::from_static("secret"));

        let transport =
            HttpClientTransport::new("http://localhost:8080").with_static_headers(headers);

        assert_eq!(
            transport.static_headers.get("X-Verber-Auth"),
            Some(&HeaderValue::from_static("secret"))
        );
    }

    #[test]
    fn test_http_client_transport_static_headers_are_debug_redacted() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Verber-Auth", HeaderValue::from_static("secret"));

        let transport =
            HttpClientTransport::new("http://localhost:8080").with_static_headers(headers);
        let debug = format!("{:?}", transport.static_headers);

        assert!(!debug.contains("secret"));
    }

    #[test]
    fn bearer_header_debug_redacts_token() {
        let header = bearer_header("secret-token").unwrap();
        let debug = format!("{header:?}");

        assert!(!debug.contains("secret-token"));
    }

    #[test]
    fn unauthorized_status_forwards_authorization_error_response() {
        let request = JSONRPCMessage::Request(JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::String("request-1".to_string()),
            request: Request {
                method: "tools/call".to_string(),
                params: None,
            },
        });
        let (tx, mut rx) = mpsc::unbounded();

        forward_status_error(&request, StatusCode::UNAUTHORIZED, &tx);

        let response = rx.try_next().unwrap().expect("response");
        let JSONRPCMessage::Response(JSONRPCResponse::Error(error)) = response else {
            panic!("expected error response");
        };
        assert_eq!(error.error.code, AUTHORIZATION_FAILED);
        assert_eq!(error.id, Some(RequestId::String("request-1".to_string())));
        assert!(
            error
                .error
                .message
                .contains("Authorization failed: HTTP 401 Unauthorized")
        );
    }

    #[tokio::test]
    async fn test_http_transport_stream() {
        let (tx1, rx1) = mpsc::unbounded();
        let (tx2, rx2) = mpsc::unbounded();
        let shutdown1 = CancellationToken::new();
        let shutdown2 = CancellationToken::new();

        let mut stream1 = HttpTransportStream {
            sender: tx1,
            receiver: rx2,
            _http_task: tokio::spawn(async {}), // Dummy task for testing
            sse_shutdown: shutdown1,
        };

        let mut stream2 = HttpTransportStream {
            sender: tx2,
            receiver: rx1,
            _http_task: tokio::spawn(async {}), // Dummy task for testing
            sse_shutdown: shutdown2,
        };

        // Test sending a message from stream1 to stream2
        let msg = JSONRPCMessage::Notification(JSONRPCNotification {
            jsonrpc: "2.0".to_string(),
            notification: Notification {
                method: "test".to_string(),
                params: None,
            },
        });

        stream1.send(msg.clone()).await.unwrap();

        let received = stream2.next().await.unwrap().unwrap().message;
        match (msg, received) {
            (JSONRPCMessage::Notification(n1), JSONRPCMessage::Notification(n2)) => {
                assert_eq!(n1.notification.method, n2.notification.method);
            }
            _ => panic!("Message type mismatch"),
        }
    }
}
