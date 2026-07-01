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
//!   SSE stream when one is open; recent events are retained for
//!   `Last-Event-ID` replay on reconnect.
//! - POSTed notifications and responses are forwarded to the connection loop
//!   and acknowledged with `202 Accepted`.
//! - DELETE terminates the session.

use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{self, Arc},
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
    routing::{delete, get, post},
};
use dashmap::DashMap;
use futures::{Sink, SinkExt, Stream, StreamExt, channel::mpsc};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::interval};
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error, info};
use uuid::Uuid;

use super::validation::{
    parse_jsonrpc_body, read_json_body, validate_json_content_type, validate_origin,
    validate_protocol_version,
};
#[cfg(feature = "auth")]
use crate::auth::server::AuthInfo;
use crate::{
    error::{Error, Result},
    schema::{JSONRPCMessage, JSONRPCNotification, JSONRPCResponse, RequestId},
    server::{HandlerFactory, NotificationFanout, Server, ServerHandle},
    transport::{IncomingMessage, Transport, TransportStream},
};

/// Extract the authenticated subject recorded by the auth middleware, if any.
#[cfg(feature = "auth")]
fn auth_subject(extensions: &http::Extensions) -> Option<String> {
    extensions
        .get::<AuthInfo>()
        .map(|info| info.subject.clone())
}

/// Without the `auth` feature no middleware records a subject.
#[cfg(not(feature = "auth"))]
fn auth_subject(_extensions: &http::Extensions) -> Option<String> {
    None
}

/// Session inactivity timeout (1 hour).
const SESSION_TIMEOUT: Duration = Duration::from_secs(3600);

/// Maximum number of concurrent sessions before initialize is refused.
const MAX_SESSIONS: usize = 1024;

/// Messages queued towards a session's connection loop before POSTs wait.
const SESSION_INCOMING_BUFFER: usize = 64;

/// Events retained per session for `Last-Event-ID` replay.
const SSE_REPLAY_CAPACITY: usize = 256;

/// Events queued on a live SSE stream before it is considered stalled and
/// dropped; the client reconnects and resumes via replay.
const SSE_STREAM_CAPACITY: usize = 1024;

/// Cross-origin policy for the HTTP transport.
///
/// Drives both the CORS response headers and Origin header validation.
#[derive(Debug, Clone, Default)]
pub enum CorsPolicy {
    /// Reject cross-origin browser requests: when an Origin header is
    /// present it must match the request Host. No CORS headers are emitted.
    #[default]
    SameOrigin,
    /// Allow requests from any origin.
    Permissive,
    /// Allow only the listed origins (exact match, e.g.
    /// `https://app.example.com`).
    AllowList(Vec<String>),
}

impl CorsPolicy {
    /// Build the CORS layer matching this policy, if one is needed.
    fn layer(&self) -> Option<CorsLayer> {
        match self {
            Self::SameOrigin => None,
            Self::Permissive => Some(CorsLayer::permissive()),
            Self::AllowList(origins) => {
                let origins: Vec<HeaderValue> = origins
                    .iter()
                    .filter_map(|origin| HeaderValue::from_str(origin).ok())
                    .collect();
                Some(
                    CorsLayer::new()
                        .allow_origin(origins)
                        .allow_methods(Any)
                        .allow_headers(Any),
                )
            }
        }
    }
}

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
    /// Cross-origin policy applied to all MCP requests.
    cors: Arc<CorsPolicy>,
}

/// A single client session and its running connection loop.
#[derive(Clone)]
struct HttpSession {
    /// Timestamp of the last observed activity for the session.
    last_activity: Arc<sync::Mutex<Instant>>,
    /// Sender feeding the session's connection loop.
    incoming_tx: mpsc::Sender<IncomingMessage>,
    /// Outbound routing shared with the session's transport.
    routes: SessionRoutes,
    /// Handle to the session's connection loop.
    handle: Arc<ServerHandle>,
    /// Authenticated subject the session is bound to.
    ///
    /// Recorded from the [`AuthInfo`] request extension at initialize when the auth
    /// middleware is installed. Subsequent requests for the session must authenticate as
    /// the same subject; mismatches are rejected with `403 Forbidden` so a session id
    /// cannot be replayed with a different identity's token.
    auth_subject: Option<String>,
}

impl HttpSession {
    /// Record activity on the session, deferring inactivity cleanup.
    fn touch(&self) {
        *self.last_activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// Forward a message to the session's connection loop.
    ///
    /// Returns false if the connection loop has shut down.
    async fn forward(&self, message: JSONRPCMessage, extensions: http::Extensions) -> bool {
        self.incoming_tx
            .clone()
            .send(IncomingMessage {
                message,
                extensions,
            })
            .await
            .is_ok()
    }
}

/// Routes outbound messages from a session's connection loop to the HTTP
/// request waiting for them, or to the standing SSE stream.
#[derive(Clone, Default)]
struct SessionRoutes {
    /// Pending request responses keyed by JSON-RPC request id.
    pending: Arc<DashMap<RequestId, oneshot::Sender<JSONRPCMessage>>>,
    /// Standing SSE stream state: live sender, event ids, and replay buffer.
    stream: Arc<sync::Mutex<StreamState>>,
}

/// SSE stream state for one session.
#[derive(Default)]
struct StreamState {
    /// Live stream sender, when a GET stream is open.
    tx: Option<mpsc::Sender<(u64, JSONRPCMessage)>>,
    /// Id assigned to the most recent event.
    last_id: u64,
    /// Recent events retained for `Last-Event-ID` replay.
    replay: VecDeque<(u64, JSONRPCMessage)>,
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

    /// Record a message in the replay buffer and forward it to the standing
    /// SSE stream, if one is open.
    fn send_to_stream(&self, message: JSONRPCMessage) {
        let mut state = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        state.last_id += 1;
        let id = state.last_id;
        state.replay.push_back((id, message.clone()));
        if state.replay.len() > SSE_REPLAY_CAPACITY {
            state.replay.pop_front();
        }
        if let Some(tx) = state.tx.as_mut()
            && tx.try_send((id, message)).is_err()
        {
            // Stalled or disconnected consumer: drop the stream. The client
            // reconnects and resumes from its Last-Event-ID via replay.
            state.tx = None;
        }
    }

    /// Install a new standing SSE stream, replacing any previous one, and
    /// queue replayable events newer than `last_event_id`.
    fn attach_stream(&self, last_event_id: Option<u64>) -> mpsc::Receiver<(u64, JSONRPCMessage)> {
        let (mut tx, rx) = mpsc::channel(SSE_STREAM_CAPACITY);
        let mut state = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = last_event_id {
            for (id, message) in &state.replay {
                if *id > last {
                    tx.try_send((*id, message.clone())).ok();
                }
            }
        }
        state.tx = Some(tx);
        rx
    }

    /// Whether a live SSE stream is currently attached.
    ///
    /// Clears the stream slot if the consumer has gone away.
    fn stream_alive(&self) -> bool {
        let mut state = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        match state.tx.as_ref() {
            Some(tx) if tx.is_closed() => {
                state.tx = None;
                false
            }
            Some(_) => true,
            None => false,
        }
    }
}

/// Transport bridging one HTTP session to its connection loop.
struct SessionTransport {
    /// Messages POSTed by the client for this session.
    incoming_rx: Option<mpsc::Receiver<IncomingMessage>>,
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
    incoming_rx: mpsc::Receiver<IncomingMessage>,
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
    /// Router containing only the MCP GET/POST/DELETE endpoint handlers.
    pub(crate) mcp_router: Router,
    /// Auxiliary top-level routes that must be merged at the application root.
    pub(crate) aux_routes: Router,
}

impl HttpServer {
    /// Create a new HTTP server around a per-session handler factory.
    pub fn new(factory: HandlerFactory, cors: CorsPolicy) -> Self {
        let state = HttpServerState {
            sessions: Arc::new(DashMap::new()),
            factory,
            shutdown: CancellationToken::new(),
            cors: Arc::new(cors),
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
            .route(mcp_route_path, delete(handle_delete))
            .with_state(self.state.clone());
        if let Some(transform) = middleware {
            mcp_router = transform(mcp_router);
        }
        let mut aux_routes = routes.unwrap_or_else(Router::new);
        if let Some(cors_layer) = self.state.cors.layer() {
            mcp_router = mcp_router.layer(cors_layer.clone());
            aux_routes = aux_routes.layer(cors_layer);
        }

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
///
/// Sessions with a live SSE stream are exempt from inactivity expiry.
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
                            let session = entry.value();
                            if session.routes.stream_alive() {
                                return false;
                            }
                            let last_active = *session
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

/// Extract the session id header, if present.
fn session_id_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Enforce session–subject binding for a request.
///
/// Returns a `403 Forbidden` response when the session was initialized by an
/// authenticated subject and the request's [`AuthInfo`] subject differs or is absent.
fn check_session_subject(session: &HttpSession, extensions: &http::Extensions) -> Option<Response> {
    let expected = session.auth_subject.as_deref()?;
    let actual = auth_subject(extensions);
    if actual.as_deref() == Some(expected) {
        None
    } else {
        Some(
            (
                StatusCode::FORBIDDEN,
                "Session is bound to a different subject",
            )
                .into_response(),
        )
    }
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

// HTTP handlers

/// Handle inbound HTTP POST JSON-RPC messages.
async fn handle_post(State(state): State<HttpServerState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let extensions = parts.extensions;

    if let Err(response) = validate_origin(&headers, &state.cors) {
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
    if let Some(response) = check_session_subject(&session, &extensions) {
        return response;
    }
    session.touch();

    match &message {
        JSONRPCMessage::Request(request) => {
            let (response_tx, response_rx) = oneshot::channel();
            session
                .routes
                .pending
                .insert(request.id.clone(), response_tx);

            if !session.forward(message, extensions).await {
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
            session.forward(message, extensions).await;
            StatusCode::ACCEPTED.into_response()
        }
        JSONRPCMessage::Response(_) => {
            session.forward(message, extensions).await;
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

    if state.sessions.len() >= MAX_SESSIONS {
        return (StatusCode::SERVICE_UNAVAILABLE, "Too many sessions").into_response();
    }

    let session_id = Uuid::new_v4().to_string();
    let (incoming_tx, incoming_rx) = mpsc::channel(SESSION_INCOMING_BUFFER);
    let routes = SessionRoutes::default();
    let (response_tx, response_rx) = oneshot::channel();
    routes.pending.insert(request.id.clone(), response_tx);

    let transport = SessionTransport {
        incoming_rx: Some(incoming_rx),
        routes: routes.clone(),
        session_id: session_id.clone(),
    };
    let server = Server::from_factory(state.factory.clone());
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
        handle: Arc::new(handle),
        auth_subject: auth_subject(&extensions),
    };
    state.sessions.insert(session_id.clone(), session.clone());

    session.forward(message, extensions).await;

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
async fn handle_get(State(state): State<HttpServerState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let headers = parts.headers;
    let extensions = parts.extensions;

    if let Err(response) = validate_origin(&headers, &state.cors) {
        return *response;
    }
    if let Err(response) = validate_protocol_version(&headers) {
        return *response;
    }

    let accepts_sse = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream") || v.contains("*/*"))
        .unwrap_or(false);
    if !accepts_sse {
        return (
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include text/event-stream",
        )
            .into_response();
    }

    let Some(session_id) = session_id_header(&headers) else {
        return (StatusCode::BAD_REQUEST, "Missing session ID").into_response();
    };
    let Some(session) = state.sessions.get(&session_id).map(|s| s.clone()) else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };
    if let Some(response) = check_session_subject(&session, &extensions) {
        return response;
    }
    session.touch();

    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    // Install the stream; a reconnecting client replaces its previous stream
    // and receives any retained events newer than its Last-Event-ID.
    let mut rx = session.routes.attach_stream(last_event_id);

    let stream = async_stream::stream! {
        while let Some((id, message)) = rx.next().await {
            let data = serde_json::to_string(&message).expect("serialize JSON-RPC message");
            yield Ok::<_, Error>(Event::default().id(id.to_string()).data(data));
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Handle session termination via HTTP DELETE.
async fn handle_delete(State(state): State<HttpServerState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let headers = parts.headers;
    let extensions = parts.extensions;

    if let Err(response) = validate_origin(&headers, &state.cors) {
        return *response;
    }

    let Some(session_id) = session_id_header(&headers) else {
        return (StatusCode::BAD_REQUEST, "Missing session ID").into_response();
    };
    let Some(session) = state.sessions.get(&session_id).map(|s| s.clone()) else {
        return (StatusCode::NOT_FOUND, "Session not found").into_response();
    };
    if let Some(response) = check_session_subject(&session, &extensions) {
        return response;
    }
    remove_session(&state, &session_id);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{JSONRPC_VERSION, JSONRPCResult, JSONRPCResultResponse, Notification};

    fn notification(method: &str) -> JSONRPCMessage {
        JSONRPCMessage::Notification(JSONRPCNotification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            notification: Notification {
                method: method.to_string(),
                params: None,
            },
        })
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
            result: JSONRPCResult {
                _meta: None,
                other: Default::default(),
            },
        }));
        routes.deliver(response);

        assert!(rx.try_recv().is_ok());
        assert!(routes.pending.is_empty());
    }

    #[test]
    fn routes_send_notifications_to_stream_with_event_ids() {
        let routes = SessionRoutes::default();
        let mut rx = routes.attach_stream(None);

        routes.deliver(notification("notifications/tools/list_changed"));
        routes.deliver(notification("notifications/prompts/list_changed"));

        let (id1, _) = rx.try_recv().unwrap();
        let (id2, _) = rx.try_recv().unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn replay_resumes_from_last_event_id() {
        let routes = SessionRoutes::default();

        // Events delivered with no stream attached are retained for replay.
        routes.deliver(notification("notifications/one"));
        routes.deliver(notification("notifications/two"));
        routes.deliver(notification("notifications/three"));

        let mut rx = routes.attach_stream(Some(1));
        let (id2, msg2) = rx.try_recv().unwrap();
        let (id3, _) = rx.try_recv().unwrap();
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        let JSONRPCMessage::Notification(n) = msg2 else {
            panic!("expected notification");
        };
        assert_eq!(n.notification.method, "notifications/two");
    }

    #[test]
    fn replay_buffer_is_bounded() {
        let routes = SessionRoutes::default();
        for _ in 0..(SSE_REPLAY_CAPACITY + 10) {
            routes.deliver(notification("notifications/n"));
        }
        let state = routes.stream.lock().unwrap();
        assert_eq!(state.replay.len(), SSE_REPLAY_CAPACITY);
    }

    #[test]
    fn stream_alive_clears_dropped_consumers() {
        let routes = SessionRoutes::default();
        let rx = routes.attach_stream(None);
        assert!(routes.stream_alive());
        drop(rx);
        assert!(!routes.stream_alive());
        // The dead slot is cleared.
        assert!(routes.stream.lock().unwrap().tx.is_none());
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
