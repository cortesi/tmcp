//! HTTP server: axum handlers with one MCP connection per session.
//!
//! Every HTTP session is a first-class MCP connection: the handler factory is
//! invoked once per session, each session runs its own connection loop, and
//! session state (request correlation, cancellation, capabilities) is fully
//! isolated between clients.
//!
//! Message routing per session:
//! - POSTed requests are correlated to their JSON-RPC response by id and
//!   answered on the POST itself with `application/json`.
//! - Server-initiated notifications and requests flow to the standing GET
//!   SSE stream when one is open, and are dropped otherwise.
//! - POSTed notifications and responses are forwarded to the connection loop
//!   and acknowledged with `202 Accepted`.

use std::{
    pin::Pin,
    result::Result as StdResult,
    sync::{
        self, Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use dashmap::DashMap;
use futures::{Sink, Stream, StreamExt, channel::mpsc};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::interval};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info};
use uuid::Uuid;

use super::validation::{
    parse_jsonrpc_body, read_json_body, validate_json_content_type, validate_origin,
};
use crate::{
    connection::ServerHandler,
    error::{Error, Result},
    schema::{
        JSONRPCMessage, JSONRPCNotification, JSONRPCResponse, LATEST_PROTOCOL_VERSION, RequestId,
    },
    server::{NotificationFanout, Server, ServerHandle},
    transport::{IncomingMessage, Transport, TransportStream},
};

/// Factory producing a fresh handler for each HTTP session.
pub type HandlerFactory = Arc<dyn Fn() -> Box<dyn ServerHandler> + Send + Sync>;

/// Session inactivity timeout (1 hour).
const SESSION_TIMEOUT: Duration = Duration::from_secs(3600);

/// An MCP server exposed over streamable HTTP.
pub struct HttpServer {
    /// State shared with the axum handlers.
    state: HttpServerState,
}

/// State shared by the axum handlers.
#[derive(Clone)]
struct HttpServerState {
    /// Active sessions keyed by server-generated session id.
    sessions: Arc<DashMap<String, HttpSession>>,
    /// Factory producing one handler per session.
    factory: HandlerFactory,
    /// Master shutdown token covering the listener and all sessions.
    shutdown: CancellationToken,
}

/// A single client session and its running connection loop.
#[derive(Clone)]
struct HttpSession {
    /// Timestamp of the last observed activity for the session.
    last_activity: Arc<sync::Mutex<Instant>>,
    /// Sender feeding the session's connection loop.
    incoming_tx: mpsc::UnboundedSender<IncomingMessage>,
    /// Outbound routing shared with the session's transport.
    routes: SessionRoutes,
    /// Monotonic id source for SSE events on the GET stream.
    event_counter: EventCounter,
    /// Handle to the session's connection loop.
    handle: Arc<ServerHandle>,
}

impl HttpSession {
    /// Record activity on the session, deferring inactivity cleanup.
    fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

/// Monotonically increasing SSE event id source.
#[derive(Clone, Debug, Default)]
struct EventCounter(Arc<AtomicU64>);

impl EventCounter {
    /// Return the next event id.
    fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Ensure the counter is at least `last_event_id`.
    fn bump_to(&self, last_event_id: u64) {
        let mut current = self.0.load(Ordering::Relaxed);
        while last_event_id > current {
            match self.0.compare_exchange(
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

/// Routes outbound messages from a session's connection loop to the HTTP
/// request waiting for them, or to the standing SSE stream.
#[derive(Clone, Default)]
struct SessionRoutes {
    /// Pending request responses keyed by JSON-RPC request id.
    pending: Arc<DashMap<RequestId, oneshot::Sender<JSONRPCMessage>>>,
    /// Sender for the standing GET SSE stream, when one is open.
    stream_tx: Arc<sync::Mutex<Option<mpsc::UnboundedSender<JSONRPCMessage>>>>,
}

impl SessionRoutes {
    /// Deliver one outbound message from the connection loop.
    fn deliver(&self, message: JSONRPCMessage) {
        if let JSONRPCMessage::Response(response) = &message {
            let id = match response {
                JSONRPCResponse::Result(result) => Some(result.id.clone()),
                JSONRPCResponse::Error(error) => error.id.clone(),
            };
            if let Some(id) = id
                && let Some((_, tx)) = self.pending.remove(&id)
            {
                tx.send(message).ok();
                return;
            }
        }
        self.send_to_stream(message);
    }

    /// Forward a message to the standing SSE stream, if one is open.
    fn send_to_stream(&self, message: JSONRPCMessage) {
        let mut guard = self.stream_tx.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(tx) => {
                if tx.unbounded_send(message).is_err() {
                    *guard = None;
                }
            }
            None => debug!("No SSE stream open; dropping server-initiated message"),
        }
    }

    /// Install a new standing SSE stream, replacing any previous one.
    fn set_stream(&self, tx: mpsc::UnboundedSender<JSONRPCMessage>) {
        *self.stream_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    }
}

/// Transport bridging one HTTP session to its connection loop.
struct SessionTransport {
    /// Messages POSTed by the client for this session.
    incoming_rx: Option<mpsc::UnboundedReceiver<IncomingMessage>>,
    /// Outbound routing back to HTTP requests and the SSE stream.
    routes: SessionRoutes,
    /// Session id, used as the connection's remote address.
    session_id: String,
}

#[async_trait]
impl Transport for SessionTransport {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    fn framed(mut self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        let incoming_rx = self
            .incoming_rx
            .take()
            .ok_or(Error::TransportDisconnected)?;
        Ok(Box::new(SessionTransportStream {
            incoming_rx,
            routes: self.routes.clone(),
        }))
    }

    fn remote_addr(&self) -> String {
        format!("http:{}", self.session_id)
    }
}

/// Stream/sink pair for one session's connection loop.
struct SessionTransportStream {
    /// Messages POSTed by the client.
    incoming_rx: mpsc::UnboundedReceiver<IncomingMessage>,
    /// Outbound routing back to HTTP requests and the SSE stream.
    routes: SessionRoutes,
}

impl Stream for SessionTransportStream {
    type Item = Result<IncomingMessage>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.incoming_rx
            .poll_next_unpin(cx)
            .map(|message| message.map(Ok))
    }
}

impl Sink<JSONRPCMessage> for SessionTransportStream {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: JSONRPCMessage) -> Result<()> {
        self.routes.deliver(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl TransportStream for SessionTransportStream {}

/// Routers returned when embedding tmcp HTTP handlers into another Axum app.
pub struct EmbeddedHttpRoutes {
    /// Router containing only the MCP GET/POST endpoint handlers.
    pub(crate) mcp_router: Router,
    /// Auxiliary top-level routes that must be merged at the application root.
    pub(crate) aux_routes: Router,
}

impl HttpServer {
    /// Create a new HTTP server around a per-session handler factory.
    pub fn new(factory: HandlerFactory) -> Self {
        let state = HttpServerState {
            sessions: Arc::new(DashMap::new()),
            factory,
            shutdown: CancellationToken::new(),
        };
        spawn_session_cleanup(&state);
        spawn_shutdown_watchdog(&state);
        Self { state }
    }

    /// Build the MCP routers mounted at `mcp_route_path`.
    pub fn routes(
        &self,
        mcp_route_path: &str,
        middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
        routes: Option<Router>,
    ) -> EmbeddedHttpRoutes {
        let mut mcp_router = Router::new()
            .route(mcp_route_path, post(handle_post))
            .route(mcp_route_path, get(handle_get))
            .with_state(self.state.clone());
        if let Some(transform) = middleware {
            mcp_router = transform(mcp_router);
        }
        mcp_router = mcp_router.layer(CorsLayer::permissive());

        let aux_routes = routes
            .unwrap_or_else(Router::new)
            .layer(CorsLayer::permissive());

        EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        }
    }

    /// Token that stops the listener and every active session.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.state.shutdown.clone()
    }

    /// Closure that fans a server notification out to every active session,
    /// subject to each session's negotiated capabilities.
    pub fn notification_fanout(&self) -> NotificationFanout {
        let sessions = self.state.sessions.clone();
        Box::new(move |notification| {
            for entry in sessions.iter() {
                entry.value().handle.send_server_notification(notification);
            }
        })
    }

    /// Bind `bind_addr` and serve `router`, returning the listener task and
    /// the actually bound address.
    pub async fn listen(
        &self,
        bind_addr: &str,
        router: Router,
    ) -> Result<(JoinHandle<()>, String)> {
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| Error::Transport(format!("Failed to bind to {bind_addr}: {e}")))?;
        let bound_addr = listener
            .local_addr()
            .map_err(|e| Error::Transport(format!("Failed to get local address: {e}")))?
            .to_string();

        info!("HTTP server listening on {}", bound_addr);
        let shutdown = self.state.shutdown.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                })
                .await
            {
                error!("HTTP server error: {}", e);
            }
        });

        Ok((task, bound_addr))
    }
}

/// Stop and remove a session, if present.
fn remove_session(state: &HttpServerState, session_id: &str) {
    if let Some((_, session)) = state.sessions.remove(session_id) {
        session.handle.signal_stop();
    }
}

/// Start the background task that expires inactive HTTP sessions.
fn spawn_session_cleanup(state: &HttpServerState) {
    let state = state.clone();

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = state.shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let now = Instant::now();
                    let expired: Vec<String> = state
                        .sessions
                        .iter()
                        .filter(|entry| {
                            let last_active = *entry
                                .value()
                                .last_activity
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            now.duration_since(last_active) > SESSION_TIMEOUT
                        })
                        .map(|entry| entry.key().clone())
                        .collect();

                    for id in expired {
                        debug!("Removing expired session: {}", id);
                        remove_session(&state, &id);
                    }
                }
            }
        }
    });
}

/// Stop all sessions when the master shutdown token fires.
fn spawn_shutdown_watchdog(state: &HttpServerState) {
    let sessions = state.sessions.clone();
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        shutdown.cancelled().await;
        for entry in sessions.iter() {
            entry.value().handle.signal_stop();
        }
        sessions.clear();
    });
}

/// Validate the MCP-Protocol-Version header, if present.
fn validate_protocol_version(headers: &HeaderMap) -> StdResult<(), Box<Response>> {
    if let Some(version) = headers.get("MCP-Protocol-Version")
        && version != LATEST_PROTOCOL_VERSION
    {
        return Err(Box::new(
            (StatusCode::BAD_REQUEST, "Unsupported protocol version").into_response(),
        ));
    }
    Ok(())
}

/// Extract the session id header, if present.
fn session_id_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Return the request id cancelled by a `notifications/cancelled` message.
fn cancelled_request_id(notification: &JSONRPCNotification) -> Option<RequestId> {
    if notification.notification.method != "notifications/cancelled" {
        return None;
    }
    let params = notification.notification.params.as_ref()?;
    let value = params.other.get("requestId")?;
    serde_json::from_value(value.clone()).ok()
}

/// Build an SSE event with a monotonically increasing id.
fn build_sse_event(counter: &EventCounter, message: &JSONRPCMessage) -> Event {
    Event::default()
        .id(counter.next().to_string())
        .data(serde_json::to_string(message).expect("serialize JSON-RPC message"))
}

// HTTP handlers

/// Handle inbound HTTP POST JSON-RPC messages.
async fn handle_post(State(state): State<HttpServerState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let extensions = parts.extensions;

    if let Err(response) = validate_origin(&headers) {
        return *response;
    }
    if let Err(response) = validate_json_content_type(&headers) {
        return *response;
    }
    if let Err(response) = validate_protocol_version(&headers) {
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

    // Initialize establishes a new session with its own connection loop. Any
    // client-supplied session id is ignored: session ids are server-generated.
    if matches!(&message, JSONRPCMessage::Request(req) if req.request.method == "initialize") {
        return handle_initialize_post(&state, message, extensions).await;
    }

    let Some(session_id) = session_id_header(&headers) else {
        return (StatusCode::BAD_REQUEST, "Missing session ID").into_response();
    };
    let Some(session) = state.sessions.get(&session_id).map(|s| s.clone()) else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };
    session.touch();

    match &message {
        JSONRPCMessage::Request(request) => {
            let (response_tx, response_rx) = oneshot::channel();
            session
                .routes
                .pending
                .insert(request.id.clone(), response_tx);

            if session
                .incoming_tx
                .unbounded_send(IncomingMessage {
                    message,
                    extensions,
                })
                .is_err()
            {
                remove_session(&state, &session_id);
                return (StatusCode::NOT_FOUND, "Session terminated").into_response();
            }

            match response_rx.await {
                Ok(response) => Json::<JSONRPCMessage>(response).into_response(),
                // The response was suppressed: the request was cancelled or
                // the session shut down before responding.
                Err(_) => StatusCode::ACCEPTED.into_response(),
            }
        }
        JSONRPCMessage::Notification(notification) => {
            // A cancellation releases the POST waiting on the cancelled
            // request: the connection loop suppresses the response entirely.
            if let Some(request_id) = cancelled_request_id(notification) {
                session.routes.pending.remove(&request_id);
            }
            session
                .incoming_tx
                .unbounded_send(IncomingMessage {
                    message,
                    extensions,
                })
                .ok();
            StatusCode::ACCEPTED.into_response()
        }
        JSONRPCMessage::Response(_) => {
            session
                .incoming_tx
                .unbounded_send(IncomingMessage {
                    message,
                    extensions,
                })
                .ok();
            StatusCode::ACCEPTED.into_response()
        }
    }
}

/// Establish a new session for an initialize request.
async fn handle_initialize_post(
    state: &HttpServerState,
    message: JSONRPCMessage,
    extensions: http::Extensions,
) -> Response {
    let JSONRPCMessage::Request(request) = &message else {
        return (StatusCode::BAD_REQUEST, "Invalid initialize message").into_response();
    };

    let session_id = Uuid::new_v4().to_string();
    let (incoming_tx, incoming_rx) = mpsc::unbounded();
    let routes = SessionRoutes::default();
    let (response_tx, response_rx) = oneshot::channel();
    routes.pending.insert(request.id.clone(), response_tx);

    let transport = SessionTransport {
        incoming_rx: Some(incoming_rx),
        routes: routes.clone(),
        session_id: session_id.clone(),
    };
    let factory = state.factory.clone();
    let server = Server::from_factory(move || factory());
    let handle = match ServerHandle::new(server, Box::new(transport)).await {
        Ok(handle) => handle,
        Err(error) => {
            error!("Failed to start HTTP session connection: {}", error);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to start session").into_response();
        }
    };

    let session = HttpSession {
        last_activity: Arc::new(sync::Mutex::new(Instant::now())),
        incoming_tx,
        routes,
        event_counter: EventCounter::default(),
        handle: Arc::new(handle),
    };
    state.sessions.insert(session_id.clone(), session.clone());

    session
        .incoming_tx
        .unbounded_send(IncomingMessage {
            message,
            extensions,
        })
        .ok();

    match response_rx.await {
        Ok(response) => {
            let failed = matches!(
                &response,
                JSONRPCMessage::Response(JSONRPCResponse::Error(_))
            );
            let mut http_response = Json::<JSONRPCMessage>(response).into_response();
            if failed {
                // No session is established when initialization fails.
                remove_session(state, &session_id);
            } else {
                http_response.headers_mut().insert(
                    "Mcp-Session-Id",
                    HeaderValue::from_str(&session_id).expect("UUID is a valid header value"),
                );
            }
            http_response
        }
        Err(_) => {
            remove_session(state, &session_id);
            (StatusCode::INTERNAL_SERVER_ERROR, "No response from server").into_response()
        }
    }
}

/// Handle inbound HTTP GET requests for the standing SSE stream.
async fn handle_get(State(state): State<HttpServerState>, headers: HeaderMap) -> Response {
    if let Err(response) = validate_origin(&headers) {
        return *response;
    }
    if let Err(response) = validate_protocol_version(&headers) {
        return *response;
    }

    let Some(session_id) = session_id_header(&headers) else {
        return (StatusCode::BAD_REQUEST, "Missing session ID").into_response();
    };
    let Some(session) = state.sessions.get(&session_id).map(|s| s.clone()) else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };
    session.touch();

    if let Some(last_event_id) = headers
        .get("Last-Event-ID")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        session.event_counter.bump_to(last_event_id);
    }

    // Install the stream; a reconnecting client replaces its previous stream.
    let (tx, mut rx) = mpsc::unbounded();
    session.routes.set_stream(tx);

    let event_counter = session.event_counter;
    let stream = async_stream::stream! {
        while let Some(msg) = rx.next().await {
            yield Ok::<_, Error>(build_sse_event(&event_counter, &msg));
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{JSONRPC_VERSION, JSONRPCResultResponse, JSONRpcResult, Notification};

    #[test]
    fn event_counter_is_monotonic_and_bumps() {
        let counter = EventCounter::default();
        assert_eq!(counter.next(), 1);
        assert_eq!(counter.next(), 2);
        counter.bump_to(10);
        assert_eq!(counter.next(), 11);
        // Bumping backwards has no effect.
        counter.bump_to(3);
        assert_eq!(counter.next(), 12);
    }

    #[test]
    fn routes_deliver_response_to_pending_request() {
        let routes = SessionRoutes::default();
        let (tx, mut rx) = oneshot::channel();
        let id = RequestId::Number(7);
        routes.pending.insert(id.clone(), tx);

        let response = JSONRPCMessage::Response(JSONRPCResponse::Result(JSONRPCResultResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: JSONRpcResult {
                _meta: None,
                other: Default::default(),
            },
        }));
        routes.deliver(response);

        assert!(rx.try_recv().is_ok());
        assert!(routes.pending.is_empty());
    }

    #[test]
    fn routes_send_notifications_to_stream() {
        let routes = SessionRoutes::default();
        let (tx, mut rx) = mpsc::unbounded();
        routes.set_stream(tx);

        let notification = JSONRPCMessage::Notification(JSONRPCNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            notification: Notification {
                method: "notifications/tools/list_changed".to_string(),
                params: None,
            },
        });
        routes.deliver(notification);

        assert!(rx.try_next().unwrap().is_some());
    }

    #[test]
    fn cancelled_request_id_parses_numeric_and_string_ids() {
        let make = |value: serde_json::Value| JSONRPCNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            notification: Notification {
                method: "notifications/cancelled".to_string(),
                params: serde_json::from_value(serde_json::json!({ "requestId": value })).unwrap(),
            },
        };
        assert_eq!(
            cancelled_request_id(&make(serde_json::json!(3))),
            Some(RequestId::Number(3))
        );
        assert_eq!(
            cancelled_request_id(&make(serde_json::json!("abc"))),
            Some(RequestId::String("abc".to_string()))
        );
    }
}
