//! HTTP server transport: axum handlers and session management.

use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        self, Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use dashmap::DashMap;
use futures::{Sink, Stream, StreamExt, channel::mpsc};
use serde_json::Value;
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
    time::{interval, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, info};
use uuid::Uuid;

use super::{
    normalize_endpoint_path,
    validation::{parse_jsonrpc_body, read_json_body, validate_json_content_type, validate_origin},
};
use crate::{
    error::{Error, Result},
    schema::{
        JSONRPCMessage, JSONRPCNotification, JSONRPCRequest, JSONRPCResponse,
        LATEST_PROTOCOL_VERSION, RequestId,
    },
    transport::{IncomingMessage, Transport, TransportStream},
};

/// Session inactivity timeout (1 hour).
const SESSION_TIMEOUT: Duration = Duration::from_secs(3600);

/// Session information for HTTP transport
#[derive(Debug, Clone)]
pub struct HttpSession {
    /// Timestamp of the last observed activity for the session.
    pub last_activity: Arc<sync::Mutex<Instant>>,
    /// Sender used to forward JSON-RPC messages to the session.
    pub sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// Receiver used to read JSON-RPC messages for the session.
    pub receiver: Arc<Mutex<mpsc::UnboundedReceiver<JSONRPCMessage>>>,
    /// Monotonic event counter for SSE messages.
    event_counter: Arc<AtomicU64>,
    /// True while a streaming SSE connection is active.
    streaming: Arc<AtomicBool>,
}

impl HttpSession {
    /// Return the next monotonically increasing SSE event id.
    fn next_event_id(&self) -> u64 {
        self.event_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Ensure the event counter is at least the provided last-event id.
    fn bump_event_id(&self, last_event_id: u64) {
        let mut current = self.event_counter.load(Ordering::Relaxed);
        while last_event_id > current {
            match self.event_counter.compare_exchange(
                current,
                last_event_id,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }
}

/// Guard that marks an SSE stream as active while held.
struct StreamingGuard {
    /// Flag tracking whether the SSE stream is active.
    flag: Arc<AtomicBool>,
}

impl StreamingGuard {
    /// Create a guard that marks streaming active until dropped.
    fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self { flag }
    }
}

impl Drop for StreamingGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// HTTP server state
#[derive(Clone)]
struct HttpServerState {
    /// Active HTTP sessions keyed by session id.
    sessions: Arc<DashMap<String, HttpSession>>,
    /// Incoming JSON-RPC messages forwarded to the server.
    incoming_tx: mpsc::UnboundedSender<(JSONRPCMessage, String, http::Extensions)>,
    /// Cancellation token for server shutdown.
    shutdown: CancellationToken,
}

/// HTTP server transport
pub struct HttpServerTransport {
    /// Address to bind the HTTP server on.
    pub bind_addr: Option<String>,
    /// Public endpoint path where MCP is served.
    endpoint_path: String,
    /// Router configured with transport endpoints.
    router: Option<Router>,
    /// Shared server state across handlers.
    state: Option<HttpServerState>,
    /// Running server task handle.
    server_handle: Option<JoinHandle<Result<()>>>,
    /// Receiver for incoming JSON-RPC messages.
    incoming_rx: Option<mpsc::UnboundedReceiver<(JSONRPCMessage, String, http::Extensions)>>,
    /// Shutdown token used to signal server termination.
    shutdown_token: Option<CancellationToken>,
}

/// Routers returned when embedding tmcp HTTP handlers into another Axum app.
pub struct EmbeddedHttpRoutes {
    /// Router containing only the MCP GET/POST endpoint handlers.
    pub(crate) mcp_router: Router,
    /// Auxiliary top-level routes that must be merged at the application root.
    pub(crate) aux_routes: Router,
}

/// Parse the Last-Event-ID header, if present.
fn parse_last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("Last-Event-ID")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

/// Apply a Last-Event-ID header to the session event counter.
fn apply_last_event_id(headers: &HeaderMap, session: &HttpSession) {
    if let Some(last_event_id) = parse_last_event_id(headers) {
        session.bump_event_id(last_event_id);
    }
}

/// Build an SSE event with a monotonically increasing id.
fn build_sse_event(session: &HttpSession, message: &JSONRPCMessage) -> Event {
    Event::default()
        .id(session.next_event_id().to_string())
        .data(serde_json::to_string(message).unwrap())
}

impl HttpServerTransport {
    /// Create a new HTTP server transport bound to the provided address.
    pub fn new(bind_addr: impl Into<String>, endpoint_path: impl Into<String>) -> Self {
        Self {
            bind_addr: Some(bind_addr.into()),
            endpoint_path: normalize_endpoint_path(endpoint_path),
            router: None,
            state: None,
            server_handle: None,
            incoming_rx: None,
            shutdown_token: None,
        }
    }

    /// Create an HTTP server transport for embedding into an existing Axum server.
    pub fn embedded(endpoint_path: impl Into<String>) -> Self {
        Self {
            bind_addr: None,
            endpoint_path: normalize_endpoint_path(endpoint_path),
            router: None,
            state: None,
            server_handle: None,
            incoming_rx: None,
            shutdown_token: None,
        }
    }

    /// Prepare the transport state and return routers for embedding.
    pub fn embed(
        &mut self,
        middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
        routes: Option<Router>,
    ) -> Result<EmbeddedHttpRoutes> {
        self.configure_routes("/", middleware, routes)
    }

    /// Start the standalone HTTP server.
    pub async fn start(
        &mut self,
        middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
        routes: Option<Router>,
    ) -> Result<()> {
        if self.server_handle.is_some() {
            return Ok(());
        }

        let EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        } = self.configure_routes(&self.endpoint_path.clone(), middleware, routes)?;
        let router = mcp_router.merge(aux_routes);

        let bind_addr = self.bind_addr.clone().ok_or_else(|| {
            Error::InvalidConfiguration("Embedded HTTP transports do not bind listeners".into())
        })?;
        let listener = TcpListener::bind(&bind_addr)
            .await
            .map_err(|e| Error::Transport(format!("Failed to bind to {bind_addr}: {e}")))?;

        // Update bind_addr with the actual address (in case port 0 was used)
        self.bind_addr = Some(
            listener
                .local_addr()
                .map_err(|e| Error::Transport(format!("Failed to get local address: {e}")))?
                .to_string(),
        );

        let bind_addr = self.bind_addr.clone().expect("bind addr set after bind");
        let shutdown = self
            .shutdown_token
            .clone()
            .expect("shutdown token initialized during route setup");

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

    /// Configure transport state and build routers.
    fn configure_routes(
        &mut self,
        mcp_route_path: &str,
        middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
        routes: Option<Router>,
    ) -> Result<EmbeddedHttpRoutes> {
        if self.incoming_rx.is_none() {
            let (incoming_tx, incoming_rx) = mpsc::unbounded();
            self.incoming_rx = Some(incoming_rx);

            let state = HttpServerState {
                sessions: Arc::new(DashMap::new()),
                incoming_tx,
                shutdown: CancellationToken::new(),
            };

            self.shutdown_token = Some(state.shutdown.clone());
            spawn_session_cleanup(&state);
            self.state = Some(state);
        }

        let state = self.state.clone().ok_or(Error::TransportDisconnected)?;

        let mut mcp_router = Router::new()
            .route(mcp_route_path, post(handle_post))
            .route(mcp_route_path, get(handle_get))
            .with_state(state);
        if let Some(transform) = middleware {
            mcp_router = transform(mcp_router);
        }
        mcp_router = mcp_router.layer(CorsLayer::permissive());

        let aux_routes = routes
            .unwrap_or_else(Router::new)
            .layer(CorsLayer::permissive());

        self.router = Some(mcp_router.clone());
        Ok(EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        })
    }
}

#[async_trait]
impl Transport for HttpServerTransport {
    async fn connect(&mut self) -> Result<()> {
        if self.incoming_rx.is_some() {
            Ok(())
        } else {
            self.start(None, None).await
        }
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
                last_activity: Arc::new(sync::Mutex::new(Instant::now())),
                sender: tx,
                receiver: Arc::new(Mutex::new(rx)),
                event_counter: Arc::new(AtomicU64::new(0)),
                streaming: Arc::new(AtomicBool::new(false)),
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
        self.bind_addr
            .clone()
            .unwrap_or_else(|| format!("embedded:{}", self.endpoint_path))
    }
}

/// Start the background task that expires inactive HTTP sessions.
fn spawn_session_cleanup(state: &HttpServerState) {
    let cleanup_sessions = state.sessions.clone();
    let cleanup_shutdown = state.shutdown.clone();

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = cleanup_shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let now = Instant::now();
                    let expired: Vec<String> = cleanup_sessions
                        .iter()
                        .filter(|entry| {
                            let last_active = *entry.value().last_activity.lock().unwrap();
                            now.duration_since(last_active) > SESSION_TIMEOUT
                        })
                        .map(|entry| entry.key().clone())
                        .collect();

                    for id in expired {
                        debug!("Removing expired session: {}", id);
                        cleanup_sessions.remove(&id);
                    }
                }
            }
        }
    });
}

/// Server-side stream implementation
struct HttpServerStream {
    /// Receiver for incoming JSON-RPC messages and session ids.
    incoming_rx: mpsc::UnboundedReceiver<(JSONRPCMessage, String, http::Extensions)>,
    /// Shared server state for routing responses.
    state: Option<HttpServerState>,
    // Track which session sent each request ID
    /// Map of request ids to originating session ids.
    request_sessions: Arc<DashMap<RequestId, String>>,
}

impl Stream for HttpServerStream {
    type Item = Result<IncomingMessage>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.incoming_rx.poll_next_unpin(cx) {
            Poll::Ready(Some((message, session_id, extensions))) => {
                // Track which session sent this request
                if let JSONRPCMessage::Request(ref req) = message {
                    self.request_sessions.insert(req.id.clone(), session_id);
                }
                Poll::Ready(Some(Ok(IncomingMessage {
                    message,
                    extensions,
                })))
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
                JSONRPCMessage::Notification(_) | JSONRPCMessage::Request(_) => {
                    if let Some(session_id) = resolve_session_id_for_message(&item, state) {
                        if let Some(session) = state.sessions.get(&session_id) {
                            session.sender.unbounded_send(item).ok();
                        }
                    } else {
                        debug!("Dropping HTTP message without session context");
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

/// Resolve a session id for outbound JSON-RPC messages.
fn resolve_session_id_for_message(
    message: &JSONRPCMessage,
    state: &HttpServerState,
) -> Option<String> {
    let session_id = match message {
        JSONRPCMessage::Request(request) => session_id_from_request(request),
        JSONRPCMessage::Notification(notification) => session_id_from_notification(notification),
        JSONRPCMessage::Response(_) => None,
    };

    session_id.or_else(|| single_session_id(state))
}

/// Return the only active session id if exactly one session exists.
fn single_session_id(state: &HttpServerState) -> Option<String> {
    if state.sessions.len() == 1 {
        state
            .sessions
            .iter()
            .next()
            .map(|entry| entry.key().clone())
    } else {
        None
    }
}

/// Extract session id from request metadata, if present.
fn session_id_from_request(request: &JSONRPCRequest) -> Option<String> {
    request.request.params.as_ref().and_then(|params| {
        params
            ._meta
            .as_ref()
            .and_then(|meta| session_id_from_meta(Some(&meta.other)))
    })
}

/// Extract session id from notification metadata, if present.
fn session_id_from_notification(notification: &JSONRPCNotification) -> Option<String> {
    notification
        .notification
        .params
        .as_ref()
        .and_then(|params| session_id_from_meta(params._meta.as_ref()))
}

/// Extract session id from a metadata map.
fn session_id_from_meta(meta: Option<&HashMap<String, Value>>) -> Option<String> {
    meta.and_then(|map| map.get("sessionId"))
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

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
async fn handle_post(State(state): State<HttpServerState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let extensions = parts.extensions;

    if let Err(response) = validate_json_content_type(&headers) {
        return *response;
    }

    let body = match read_json_body(body).await {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let message = match parse_jsonrpc_body(&body) {
        Ok(message) => message,
        Err(response) => return *response,
    };

    debug!("HTTP server received POST request: {:?}", message);

    if let Err(response) = validate_origin(&headers) {
        return *response;
    }

    // Validate protocol version
    if let Some(version) = headers.get("MCP-Protocol-Version")
        && version != LATEST_PROTOCOL_VERSION
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
            last_activity: Arc::new(sync::Mutex::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
            event_counter: Arc::new(AtomicU64::new(0)),
            streaming: Arc::new(AtomicBool::new(false)),
        };

        state
            .sessions
            .insert(new_session_id.clone(), session.clone());

        // Forward to server logic
        state
            .incoming_tx
            .unbounded_send((message, new_session_id.clone(), extensions))
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

    let session = if let Some(session) = state.sessions.get(&session_id) {
        *session.last_activity.lock().unwrap() = Instant::now();
        session.clone()
    } else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };

    match &message {
        JSONRPCMessage::Request(_) => {
            // Forward to server logic
            state
                .incoming_tx
                .unbounded_send((message, session_id.clone(), extensions))
                .ok();

            if session.streaming.load(Ordering::SeqCst) {
                return StatusCode::ACCEPTED.into_response();
            }

            // Check if client accepts SSE
            let accepts_sse = headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.contains("text/event-stream"))
                .unwrap_or(false);

            if accepts_sse {
                // Return SSE stream for response
                apply_last_event_id(&headers, &session);
                let receiver = session.receiver.clone();
                let stream = async_stream::stream! {
                    let mut receiver = receiver.lock().await;
                    while let Some(msg) = receiver.next().await {
                        yield Ok::<_, Error>(build_sse_event(&session, &msg));

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
                    Ok(None) => (StatusCode::INTERNAL_SERVER_ERROR, "No response").into_response(),
                    Err(_) => (StatusCode::REQUEST_TIMEOUT, "Request timeout").into_response(),
                }
            }
        }
        JSONRPCMessage::Response(_) | JSONRPCMessage::Notification(_) => {
            // Forward to server logic
            state
                .incoming_tx
                .unbounded_send((message, session_id, extensions))
                .ok();
            StatusCode::ACCEPTED.into_response()
        }
    }
}

/// Handle inbound HTTP GET requests for SSE streams.
async fn handle_get(State(state): State<HttpServerState>, headers: HeaderMap) -> Response {
    if let Err(response) = validate_origin(&headers) {
        return *response;
    }

    // Validate protocol version
    if let Some(version) = headers.get("MCP-Protocol-Version")
        && version != LATEST_PROTOCOL_VERSION
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
    let session = if let Some(session) = state.sessions.get(&session_id) {
        apply_last_event_id(&headers, &session);
        session.clone()
    } else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };
    let receiver = session.receiver.clone();
    let streaming = session.streaming.clone();
    let stream = async_stream::stream! {
        let _guard = StreamingGuard::new(streaming);
        let mut receiver = receiver.lock().await;
        while let Some(msg) = receiver.next().await {
            yield Ok::<_, Error>(build_sse_event(&session, &msg));
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_http_server_transport_creation() {
        let transport = HttpServerTransport::new("127.0.0.1:8080", "/");
        assert_eq!(transport.bind_addr, Some("127.0.0.1:8080".to_string()));
        assert_eq!(transport.endpoint_path, "/");
    }

    #[tokio::test]
    async fn test_session_management() {
        let (tx, rx) = mpsc::unbounded();

        let session = HttpSession {
            last_activity: Arc::new(sync::Mutex::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
            event_counter: Arc::new(AtomicU64::new(0)),
            streaming: Arc::new(AtomicBool::new(false)),
        };

        // Test that we can update last activity
        let before = *session.last_activity.lock().unwrap();
        sleep(Duration::from_millis(10)).await;
        *session.last_activity.lock().unwrap() = Instant::now();
        let after = *session.last_activity.lock().unwrap();
        assert!(after > before);
    }

    #[test]
    fn test_apply_last_event_id_advances_counter() {
        let (tx, rx) = mpsc::unbounded();
        let session = HttpSession {
            last_activity: Arc::new(sync::Mutex::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
            event_counter: Arc::new(AtomicU64::new(0)),
            streaming: Arc::new(AtomicBool::new(false)),
        };

        let mut headers = HeaderMap::new();
        headers.insert("Last-Event-ID", HeaderValue::from_static("5"));
        apply_last_event_id(&headers, &session);

        assert_eq!(session.next_event_id(), 6);
    }
}
