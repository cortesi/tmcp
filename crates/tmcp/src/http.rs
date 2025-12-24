use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use dashmap::DashMap;
use eventsource_stream::Eventsource;
use futures::{Sink, Stream, StreamExt, channel::mpsc};
use reqwest::Client as HttpClient;
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock, oneshot},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::{
    auth::OAuth2Client,
    error::{Error, Result},
    schema::{JSONRPCMessage, JSONRPCResponse, RequestId},
    transport::{Transport, TransportStream},
};

/// MCP protocol version advertised for HTTP transport.
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
/// Default HTTP client timeout for transport requests.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Session information for HTTP transport
#[derive(Debug, Clone)]
pub struct HttpSession {
    /// Timestamp of the last observed activity for the session.
    pub last_activity: Arc<RwLock<Instant>>,
    /// Sender used to forward JSON-RPC messages to the session.
    pub sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// Receiver used to read JSON-RPC messages for the session.
    pub receiver: Arc<Mutex<mpsc::UnboundedReceiver<JSONRPCMessage>>>,
}

/// HTTP server state
#[derive(Clone)]
struct HttpServerState {
    /// Active HTTP sessions keyed by session id.
    sessions: Arc<DashMap<String, HttpSession>>,
    /// Incoming JSON-RPC messages forwarded to the server.
    incoming_tx: mpsc::UnboundedSender<(JSONRPCMessage, String)>,
    /// Cancellation token for server shutdown.
    shutdown: CancellationToken,
}

/// HTTP client transport
#[doc(hidden)]
pub struct HttpClientTransport {
    /// Endpoint URL for HTTP transport.
    endpoint: String,
    /// HTTP client used to send requests.
    client: HttpClient,
    /// Session identifier returned by the server.
    session_id: Option<String>,
    /// Sender half for outbound JSON-RPC messages.
    sender: Option<mpsc::UnboundedSender<JSONRPCMessage>>,
    /// Receiver half for inbound JSON-RPC messages.
    receiver: Option<mpsc::UnboundedReceiver<JSONRPCMessage>>,
    /// Task handle for the SSE receive loop.
    sse_handle: Option<JoinHandle<()>>,
    /// OAuth client for attaching bearer tokens.
    oauth_client: Option<Arc<OAuth2Client>>,
}

/// HTTP server transport
#[doc(hidden)]
pub struct HttpServerTransport {
    /// Address to bind the HTTP server on.
    pub bind_addr: String,
    /// Router configured with transport endpoints.
    router: Option<Router>,
    /// Shared server state across handlers.
    state: Option<HttpServerState>,
    /// Running server task handle.
    server_handle: Option<JoinHandle<Result<()>>>,
    /// Receiver for incoming JSON-RPC messages.
    incoming_rx: Option<mpsc::UnboundedReceiver<(JSONRPCMessage, String)>>,
    /// Shutdown token used to signal server termination.
    shutdown_token: Option<CancellationToken>,
}

/// Stream wrapper for HTTP transport
struct HttpTransportStream {
    /// Sender for outgoing JSON-RPC messages.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// Receiver for incoming JSON-RPC messages.
    receiver: mpsc::UnboundedReceiver<JSONRPCMessage>,
    /// Join handle for the background HTTP task.
    _http_task: JoinHandle<()>,
}

impl Drop for HttpTransportStream {
    fn drop(&mut self) {
        // Cancel the HTTP task when the stream is dropped
        self._http_task.abort();
    }
}

/// Capture a session id from response headers for initialize requests.
fn update_session_id(is_initialize: bool, headers: &HeaderMap, session_id: &mut Option<String>) {
    if !is_initialize {
        return;
    }

    let Some(sid) = headers.get("Mcp-Session-Id") else {
        return;
    };

    let Ok(sid_str) = sid.to_str() else {
        return;
    };

    *session_id = Some(sid_str.to_string());
    debug!("Got session ID: {}", sid_str);
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

/// Handle an HTTP response for a single outbound JSON-RPC message.
async fn handle_http_response(
    msg: &JSONRPCMessage,
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
) {
    if !validate_status(response.status()) {
        return;
    }

    if !expects_response(msg) {
        return;
    }

    forward_response(response, sender).await;
}

impl Drop for HttpClientTransport {
    fn drop(&mut self) {
        // Abort any running SSE handle
        if let Some(handle) = self.sse_handle.take() {
            handle.abort();
        }
    }
}

impl HttpClientTransport {
    /// Create a new HTTP client transport for the provided endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: HttpClient::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("Failed to create HTTP client"),
            session_id: None,
            sender: None,
            receiver: None,
            sse_handle: None,
            oauth_client: None,
        }
    }

    /// Attach an OAuth client used to fetch bearer tokens for requests.
    pub fn with_oauth(mut self, oauth_client: Arc<OAuth2Client>) -> Self {
        self.oauth_client = Some(oauth_client);
        self
    }

    /// Connect to SSE stream for receiving server messages
    async fn connect_sse(&mut self) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            "MCP-Protocol-Version",
            HeaderValue::from_static(MCP_PROTOCOL_VERSION),
        );

        if let Some(session_id) = &self.session_id {
            headers.insert(
                "Mcp-Session-Id",
                HeaderValue::from_str(session_id)
                    .map_err(|_| Error::Transport("Invalid session ID".into()))?,
            );
        }

        // Add OAuth authorization header if available
        if let Some(oauth_client) = &self.oauth_client {
            let token = oauth_client.get_valid_token().await?;
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| Error::Transport("Invalid authorization token".into()))?,
            );
        }

        let response = self
            .client
            .get(&self.endpoint)
            .headers(headers)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("Failed to connect SSE: {e}")))?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            // Server doesn't support SSE endpoint
            return Ok(());
        }

        if !response.status().is_success() {
            return Err(Error::Transport(format!(
                "SSE connection failed with status: {}",
                response.status()
            )));
        }

        let stream = response.bytes_stream().eventsource();
        let sender = self.sender.as_ref().unwrap().clone();

        // Spawn task to handle SSE events
        let handle = tokio::spawn(async move {
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(event) => {
                        if let Ok(msg) = serde_json::from_str::<JSONRPCMessage>(&event.data)
                            && sender.unbounded_send(msg).is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("SSE error: {:?}", e);
                        break;
                    }
                }
            }
        });

        self.sse_handle = Some(handle);
        Ok(())
    }
}

#[async_trait]
impl Transport for HttpClientTransport {
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to HTTP endpoint: {}", self.endpoint);

        // Create channels for bidirectional communication
        let (tx, rx) = mpsc::unbounded();
        self.sender = Some(tx);
        self.receiver = Some(rx);

        // Try to establish SSE connection for server-initiated messages
        if let Err(e) = self.connect_sse().await {
            debug!("SSE connection failed (server may not support it): {}", e);
        }

        Ok(())
    }

    fn framed(mut self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        let sender = self.sender.take().ok_or(Error::TransportDisconnected)?;
        let receiver = self.receiver.take().ok_or(Error::TransportDisconnected)?;

        // Create a task to handle sending messages via HTTP
        let endpoint = self.endpoint.clone();
        let client = self.client.clone();
        let mut session_id = self.session_id.clone();
        let oauth_client = self.oauth_client.clone();

        let (http_tx, mut http_rx) = mpsc::unbounded::<JSONRPCMessage>();
        let sender_clone = sender;

        let http_task = tokio::spawn(async move {
            while let Some(msg) = http_rx.next().await {
                debug!("HTTP client sending message: {:?}", msg);

                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
                headers.insert(
                    "MCP-Protocol-Version",
                    HeaderValue::from_static(MCP_PROTOCOL_VERSION),
                );

                if let Some(ref sid) = session_id {
                    headers.insert("Mcp-Session-Id", HeaderValue::from_str(sid).unwrap());
                }

                // Add OAuth authorization header if available
                if let Some(oauth_client) = &oauth_client {
                    match oauth_client.get_valid_token().await {
                        Ok(token) => {
                            headers.insert(
                                header::AUTHORIZATION,
                                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
                            );
                        }
                        Err(e) => {
                            error!("Failed to get OAuth token: {}", e);
                            continue;
                        }
                    }
                }

                // Check if this is an initialize request to capture session ID
                let is_initialize = matches!(&msg, JSONRPCMessage::Request(req) if req.request.method == "initialize");

                match client
                    .post(&endpoint)
                    .headers(headers)
                    .json(&msg)
                    .send()
                    .await
                {
                    Ok(response) => {
                        debug!("HTTP response status: {}", response.status());
                        update_session_id(is_initialize, response.headers(), &mut session_id);
                        handle_http_response(&msg, response, &sender_clone).await;
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
        }))
    }

    fn remote_addr(&self) -> String {
        self.endpoint.clone()
    }
}

impl HttpServerTransport {
    /// Create a new HTTP server transport bound to the provided address.
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            router: None,
            state: None,
            server_handle: None,
            incoming_rx: None,
            shutdown_token: None,
        }
    }

    /// Start the HTTP server
    pub async fn start(&mut self) -> Result<()> {
        // If already started, just return
        if self.server_handle.is_some() {
            return Ok(());
        }

        let (incoming_tx, incoming_rx) = mpsc::unbounded();
        self.incoming_rx = Some(incoming_rx);

        let state = HttpServerState {
            sessions: Arc::new(DashMap::new()),
            incoming_tx,
            shutdown: CancellationToken::new(),
        };

        self.state = Some(state.clone());
        self.shutdown_token = Some(state.shutdown.clone());

        let router = Router::new()
            .route("/", post(handle_post))
            .route("/", get(handle_get))
            .layer(CorsLayer::permissive())
            .with_state(state.clone());

        self.router = Some(router.clone());

        let listener = TcpListener::bind(&self.bind_addr).await.map_err(|e| {
            Error::Transport(format!("Failed to bind to {}: {}", self.bind_addr, e))
        })?;

        // Update bind_addr with the actual address (in case port 0 was used)
        self.bind_addr = listener
            .local_addr()
            .map_err(|e| Error::Transport(format!("Failed to get local address: {e}")))?
            .to_string();

        let bind_addr = self.bind_addr.clone();
        let shutdown = state.shutdown.clone();

        // Create a channel to signal when the server is actually ready
        let (ready_tx, ready_rx) = oneshot::channel();

        // Clone for the ready signal
        let bind_addr_clone = bind_addr.clone();

        let server_handle = tokio::spawn(async move {
            info!("HTTP server starting on {}", bind_addr);

            // Signal readiness immediately - axum::serve will start accepting connections
            // as soon as it's called with the already-bound listener
            ready_tx.send(()).ok();

            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                })
                .await
                .map_err(|e| Error::Transport(format!("Server error: {e}")))
        });

        self.server_handle = Some(server_handle);

        // Wait for the ready signal
        ready_rx
            .await
            .map_err(|_| Error::Transport("Server failed to start".into()))?;

        // Give a small delay to ensure axum is fully ready
        sleep(Duration::from_millis(100)).await;

        info!("HTTP server ready on {}", bind_addr_clone);

        Ok(())
    }
}

#[async_trait]
impl Transport for HttpServerTransport {
    async fn connect(&mut self) -> Result<()> {
        self.start().await
    }

    fn framed(mut self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        // Take ownership of shutdown_token to prevent Drop from cancelling it
        let _shutdown_token = self.shutdown_token.take();

        let incoming_rx = self
            .incoming_rx
            .take()
            .ok_or(Error::TransportDisconnected)?;

        // Create a session for the server side
        let session_id = Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::unbounded();

        if let Some(state) = &self.state {
            let session = HttpSession {
                last_activity: Arc::new(RwLock::new(Instant::now())),
                sender: tx,
                receiver: Arc::new(Mutex::new(rx)),
            };

            state.sessions.insert(session_id, session);
        }

        // Create a stream that merges incoming messages with session-specific messages
        let stream = HttpServerStream {
            incoming_rx,
            state: self.state.clone(),
            request_sessions: Arc::new(DashMap::new()),
        };

        Ok(Box::new(stream))
    }

    fn remote_addr(&self) -> String {
        self.bind_addr.clone()
    }
}

/// Server-side stream implementation
struct HttpServerStream {
    /// Receiver for incoming JSON-RPC messages and session ids.
    incoming_rx: mpsc::UnboundedReceiver<(JSONRPCMessage, String)>,
    /// Shared server state for routing responses.
    state: Option<HttpServerState>,
    // Track which session sent each request ID
    /// Map of request ids to originating session ids.
    request_sessions: Arc<DashMap<RequestId, String>>,
}

impl Stream for HttpServerStream {
    type Item = Result<JSONRPCMessage>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.incoming_rx.poll_next_unpin(cx) {
            Poll::Ready(Some((msg, session_id))) => {
                // Track which session sent this request
                if let JSONRPCMessage::Request(ref req) = msg {
                    self.request_sessions.insert(req.id.clone(), session_id);
                }
                Poll::Ready(Some(Ok(msg)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Sink<JSONRPCMessage> for HttpServerStream {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: JSONRPCMessage) -> Result<()> {
        if let Some(state) = &self.state {
            match &item {
                JSONRPCMessage::Response(resp) => {
                    // Route response to the correct session
                    let response_id = match resp {
                        JSONRPCResponse::Result(result) => Some(result.id.clone()),
                        JSONRPCResponse::Error(error) => error.id.clone(),
                    };

                    if let Some(response_id) = response_id
                        && let Some((_, session_id)) = self.request_sessions.remove(&response_id)
                        && let Some(session) = state.sessions.get(&session_id)
                    {
                        session.sender.unbounded_send(item).ok();
                    }
                }
                JSONRPCMessage::Notification(_) => {
                    // Broadcast notifications to all sessions
                    for session in state.sessions.iter() {
                        session.sender.unbounded_send(item.clone()).ok();
                    }
                }
                _ => {
                    // For other message types, broadcast to all
                    for session in state.sessions.iter() {
                        session.sender.unbounded_send(item.clone()).ok();
                    }
                }
            }
        }
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl TransportStream for HttpServerStream {}

impl Drop for HttpServerTransport {
    fn drop(&mut self) {
        // Trigger shutdown when transport is dropped
        if let Some(token) = &self.shutdown_token {
            token.cancel();
        }
    }
}

// HTTP handlers

/// Handle inbound HTTP POST JSON-RPC messages.
async fn handle_post(
    State(state): State<HttpServerState>,
    headers: HeaderMap,
    Json(message): Json<JSONRPCMessage>,
) -> Response {
    debug!("HTTP server received POST request: {:?}", message);

    // Validate protocol version
    if let Some(version) = headers.get("MCP-Protocol-Version")
        && version != MCP_PROTOCOL_VERSION
    {
        return (StatusCode::BAD_REQUEST, "Unsupported protocol version").into_response();
    }

    let session_id = headers
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Handle initialization specially
    if matches!(&message, JSONRPCMessage::Request(req) if req.request.method == "initialize") {
        let new_session_id = session_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let (tx, rx) = mpsc::unbounded();

        let session = HttpSession {
            last_activity: Arc::new(RwLock::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
        };

        state
            .sessions
            .insert(new_session_id.clone(), session.clone());

        // Forward to server logic
        state
            .incoming_tx
            .unbounded_send((message, new_session_id.clone()))
            .ok();

        // Wait for the actual response from the server with timeout
        let receiver = session.receiver.clone();

        let response = timeout(Duration::from_secs(5), async move {
            let mut receiver = receiver.lock().await;
            receiver.next().await
        })
        .await;

        match response {
            Ok(Some(response)) => {
                let mut http_response = Json::<JSONRPCMessage>(response).into_response();

                // Add session ID header
                http_response.headers_mut().insert(
                    "Mcp-Session-Id",
                    HeaderValue::from_str(&new_session_id).unwrap(),
                );

                return http_response;
            }
            Ok(None) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "No response from server")
                    .into_response();
            }
            Err(_) => {
                return (StatusCode::REQUEST_TIMEOUT, "Initialization timeout").into_response();
            }
        }
    }

    // For other messages, validate session
    let session_id = match session_id {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Missing session ID").into_response(),
    };

    if !state.sessions.contains_key(&session_id) {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    }

    // Update last activity
    if let Some(session) = state.sessions.get(&session_id) {
        *session.last_activity.write().await = Instant::now();
    }

    match &message {
        JSONRPCMessage::Request(_) => {
            // Forward to server logic
            state
                .incoming_tx
                .unbounded_send((message, session_id.clone()))
                .ok();

            // Check if client accepts SSE
            let accepts_sse = headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("text/event-stream"))
                .unwrap_or(false);

            if accepts_sse {
                // Return SSE stream for response
                let receiver = if let Some(session) = state.sessions.get(&session_id) {
                    session.receiver.clone()
                } else {
                    return (StatusCode::NOT_FOUND, "Session not found").into_response();
                };

                let stream = async_stream::stream! {
                    let mut receiver = receiver.lock().await;
                    while let Some(msg) = receiver.next().await {
                        yield Ok::<_, Error>(Event::default().data(
                            serde_json::to_string(&msg).unwrap()
                        ));

                        // If this is a response to our request, close the stream
                        if matches!(&msg, JSONRPCMessage::Response(_)) {
                            break;
                        }
                    }
                };

                Sse::new(stream)
                    .keep_alive(KeepAlive::default())
                    .into_response()
            } else {
                // Wait for response and return directly
                if let Some(session) = state.sessions.get(&session_id) {
                    let receiver = session.receiver.clone();

                    let response = timeout(
                        Duration::from_secs(30), // 30 second timeout for requests
                        async move {
                            let mut receiver = receiver.lock().await;
                            receiver.next().await
                        },
                    )
                    .await;

                    match response {
                        Ok(Some(response)) => Json::<JSONRPCMessage>(response).into_response(),
                        Ok(None) => {
                            (StatusCode::INTERNAL_SERVER_ERROR, "No response").into_response()
                        }
                        Err(_) => (StatusCode::REQUEST_TIMEOUT, "Request timeout").into_response(),
                    }
                } else {
                    (StatusCode::NOT_FOUND, "Session not found").into_response()
                }
            }
        }
        JSONRPCMessage::Response(_) | JSONRPCMessage::Notification(_) => {
            // Forward to server logic
            state.incoming_tx.unbounded_send((message, session_id)).ok();
            StatusCode::ACCEPTED.into_response()
        }
    }
}

/// Handle inbound HTTP GET requests for SSE streams.
async fn handle_get(State(state): State<HttpServerState>, headers: HeaderMap) -> Response {
    // Validate protocol version
    if let Some(version) = headers.get("MCP-Protocol-Version")
        && version != MCP_PROTOCOL_VERSION
    {
        return (StatusCode::BAD_REQUEST, "Unsupported protocol version").into_response();
    }

    let session_id = headers
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let session_id = match session_id {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Missing session ID").into_response(),
    };

    // Clone the receiver to avoid lifetime issues
    let receiver = if let Some(session) = state.sessions.get(&session_id) {
        session.receiver.clone()
    } else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };

    let stream = async_stream::stream! {
        let mut receiver = receiver.lock().await;
        while let Some(msg) = receiver.next().await {
            yield Ok::<_, Error>(Event::default().data(
                serde_json::to_string(&msg).unwrap()
            ));
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

impl Stream for HttpTransportStream {
    type Item = Result<JSONRPCMessage>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_next_unpin(cx) {
            Poll::Ready(Some(msg)) => Poll::Ready(Some(Ok(msg))),
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
    use crate::schema::{JSONRPCNotification, Notification};

    #[tokio::test]
    async fn test_http_client_transport_creation() {
        let transport = HttpClientTransport::new("http://localhost:8080");
        assert_eq!(transport.endpoint, "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_http_server_transport_creation() {
        let transport = HttpServerTransport::new("127.0.0.1:8080");
        assert_eq!(transport.bind_addr, "127.0.0.1:8080");
    }

    #[tokio::test]
    async fn test_session_management() {
        let (tx, rx) = mpsc::unbounded();

        let session = HttpSession {
            last_activity: Arc::new(RwLock::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
        };

        // Test that we can update last activity
        let before = *session.last_activity.read().await;
        sleep(Duration::from_millis(10)).await;
        *session.last_activity.write().await = Instant::now();
        let after = *session.last_activity.read().await;
        assert!(after > before);
    }

    #[tokio::test]
    async fn test_http_transport_stream() {
        let (tx1, rx1) = mpsc::unbounded();
        let (tx2, rx2) = mpsc::unbounded();

        let mut stream1 = HttpTransportStream {
            sender: tx1,
            receiver: rx2,
            _http_task: tokio::spawn(async {}), // Dummy task for testing
        };

        let mut stream2 = HttpTransportStream {
            sender: tx2,
            receiver: rx1,
            _http_task: tokio::spawn(async {}), // Dummy task for testing
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

        let received = stream2.next().await.unwrap().unwrap();
        match (msg, received) {
            (JSONRPCMessage::Notification(n1), JSONRPCMessage::Notification(n2)) => {
                assert_eq!(n1.notification.method, n2.notification.method);
            }
            _ => panic!("Message type mismatch"),
        }
    }
}
