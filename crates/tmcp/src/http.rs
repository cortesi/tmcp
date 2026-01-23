use std::{
    collections::HashMap,
    pin::Pin,
    result::Result as StdResult,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header, uri::Authority},
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
use serde_json::Value;
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock, oneshot},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};
use url::Url;
use uuid::Uuid;

use crate::{
    auth::OAuth2Client,
    error::{Error, Result},
    schema::{
        JSONRPCMessage, JSONRPCNotification, JSONRPCRequest, JSONRPCResponse,
        LATEST_PROTOCOL_VERSION, RequestId,
    },
    transport::{Transport, TransportStream},
};
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
    /// Monotonic event counter for SSE messages.
    event_counter: Arc<AtomicU64>,
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
    session_id: Arc<Mutex<Option<String>>>,
    /// Last observed SSE event id.
    last_event_id: Arc<Mutex<Option<String>>>,
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
        return;
    }

    if !expects_response(msg) {
        return;
    }

    if response_is_sse(&response) {
        forward_sse_response(response, sender, last_event_id).await;
    } else {
        forward_response(response, sender).await;
    }
}

/// Result type used for origin validation checks.
type OriginResult = StdResult<(), Box<Response>>;

/// Validate the Origin header against the request Host when present.
fn validate_origin(headers: &HeaderMap) -> OriginResult {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(());
    };

    let origin_str = origin
        .to_str()
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if origin_str == "null" {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    let origin_url = Url::parse(origin_str)
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if !matches!(origin_url.scheme(), "http" | "https") {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    let host_header = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    let authority = host_header
        .parse::<Authority>()
        .map_err(|_| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;

    let origin_host = origin_url
        .host_str()
        .ok_or_else(|| Box::new((StatusCode::FORBIDDEN, "Invalid Origin").into_response()))?;
    if !origin_host.eq_ignore_ascii_case(authority.host()) {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    if let Some(expected_port) = authority.port_u16() {
        if origin_url.port_or_known_default() != Some(expected_port) {
            return Err(Box::new(
                (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
            ));
        }
    } else if origin_url.port().is_some() {
        return Err(Box::new(
            (StatusCode::FORBIDDEN, "Invalid Origin").into_response(),
        ));
    }

    Ok(())
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

impl HttpClientTransport {
    /// Create a new HTTP client transport for the provided endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: HttpClient::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("Failed to create HTTP client"),
            session_id: Arc::new(Mutex::new(None)),
            last_event_id: Arc::new(Mutex::new(None)),
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

    /// Connect to SSE stream for receiving server messages
    async fn connect_sse(
        client: HttpClient,
        endpoint: String,
        session_id: Arc<Mutex<Option<String>>>,
        last_event_id: Arc<Mutex<Option<String>>>,
        sender: mpsc::UnboundedSender<JSONRPCMessage>,
        oauth_client: Option<Arc<OAuth2Client>>,
        shutdown: CancellationToken,
    ) -> Result<SseOutcome> {
        let Some(session_id_value) = session_id.lock().await.clone() else {
            return Ok(SseOutcome::NoSession);
        };

        let mut headers = HeaderMap::new();
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

        if let Some(last_event_id_value) = last_event_id.lock().await.clone() {
            headers.insert(
                "Last-Event-ID",
                HeaderValue::from_str(&last_event_id_value)
                    .map_err(|_| Error::Transport("Invalid Last-Event-ID".into()))?,
            );
        }

        // Add OAuth authorization header if available
        if let Some(oauth_client) = &oauth_client {
            let token = oauth_client.get_valid_token().await?;
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| Error::Transport("Invalid authorization token".into()))?,
            );
        }

        let response = client
            .get(&endpoint)
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
                _ = shutdown.cancelled() => break,
                event = stream.next() => {
                    match event {
                        Some(Ok(event)) => {
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
        let outcome = HttpClientTransport::connect_sse(
            context.client,
            context.endpoint,
            context.session_id,
            context.last_event_id,
            context.sender,
            context.oauth_client,
            context.shutdown,
        )
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
        let oauth_client = self.oauth_client.clone();
        let sse_state = self.sse_state.clone();
        let sse_shutdown = self.sse_shutdown.clone();

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
                headers.insert(
                    header::ACCEPT,
                    HeaderValue::from_static("application/json, text/event-stream"),
                );
                headers.insert(
                    "MCP-Protocol-Version",
                    HeaderValue::from_static(LATEST_PROTOCOL_VERSION),
                );

                if let Some(ref sid) = *session_id.lock().await {
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
                        update_session_id(is_initialize, response.headers(), &session_id).await;
                        handle_http_response(&msg, response, &sender_clone, &last_event_id).await;
                        maybe_start_sse(SseStartContext {
                            client: client.clone(),
                            endpoint: endpoint.clone(),
                            session_id: session_id.clone(),
                            last_event_id: last_event_id.clone(),
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
                event_counter: Arc::new(AtomicU64::new(0)),
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
async fn handle_post(
    State(state): State<HttpServerState>,
    headers: HeaderMap,
    Json(message): Json<JSONRPCMessage>,
) -> Response {
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
            last_activity: Arc::new(RwLock::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
            event_counter: Arc::new(AtomicU64::new(0)),
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
                let session = if let Some(session) = state.sessions.get(&session_id) {
                    apply_last_event_id(&headers, &session);
                    session.clone()
                } else {
                    return (StatusCode::NOT_FOUND, "Session not found").into_response();
                };
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
    let stream = async_stream::stream! {
        let mut receiver = receiver.lock().await;
        while let Some(msg) = receiver.next().await {
            yield Ok::<_, Error>(build_sse_event(&session, &msg));
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
            event_counter: Arc::new(AtomicU64::new(0)),
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

        let received = stream2.next().await.unwrap().unwrap();
        match (msg, received) {
            (JSONRPCMessage::Notification(n1), JSONRPCMessage::Notification(n2)) => {
                assert_eq!(n1.notification.method, n2.notification.method);
            }
            _ => panic!("Message type mismatch"),
        }
    }

    #[test]
    fn test_validate_origin_allows_same_host() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        assert!(validate_origin(&headers).is_ok());
    }

    #[test]
    fn test_validate_origin_rejects_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));

        let response = validate_origin(&headers).unwrap_err();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_apply_last_event_id_advances_counter() {
        let (tx, rx) = mpsc::unbounded();
        let session = HttpSession {
            last_activity: Arc::new(RwLock::new(Instant::now())),
            sender: tx,
            receiver: Arc::new(Mutex::new(rx)),
            event_counter: Arc::new(AtomicU64::new(0)),
        };

        let mut headers = HeaderMap::new();
        headers.insert("Last-Event-ID", HeaderValue::from_static("5"));
        apply_last_event_id(&headers, &session);

        assert_eq!(session.next_event_id(), 6);
    }
}
