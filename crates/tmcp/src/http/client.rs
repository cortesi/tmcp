//! HTTP client transport: streamable HTTP with SSE support.
//!
//! Outbound messages are sent as HTTP POSTs with bounded concurrency:
//! requests share a concurrency limit while notifications (notably
//! `notifications/cancelled`) bypass it, so a slow call can never block
//! cancellation. A persistent manager task keeps the standing SSE GET stream
//! connected with exponential backoff, reconnecting with `Last-Event-ID` so
//! the server can replay missed events.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use eventsource_stream::Eventsource;
use futures::{Sink, Stream, StreamExt, channel::mpsc};
use reqwest::Client as HttpClient;
use tokio::{
    sync::{Mutex, Notify, Semaphore},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::{
    auth::{OAuth2Client, util::bearer_header},
    error::{Error, Result},
    schema::{
        AUTHORIZATION_FAILED, ErrorObject, INTERNAL_ERROR, JSONRPC_VERSION, JSONRPCErrorResponse,
        JSONRPCMessage, JSONRPCResponse, LATEST_PROTOCOL_VERSION,
    },
    transport::{IncomingMessage, Transport, TransportStream},
};

/// Idle read timeout between response chunks; SSE keep-alives reset it, so
/// long-lived streams survive while dead connections are detected.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for establishing a TCP/TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Initial delay between SSE reconnect attempts.
const SSE_RECONNECT_MIN: Duration = Duration::from_millis(250);

/// Maximum delay between SSE reconnect attempts.
const SSE_RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Maximum concurrently in-flight outbound requests.
const MAX_CONCURRENT_REQUESTS: usize = 8;

/// HTTP client transport
pub struct HttpClientTransport {
    /// Endpoint URL for HTTP transport.
    endpoint: String,
    /// HTTP client used to send requests.
    client: HttpClient,
    /// Session identifier returned by the server.
    session_id: Arc<Mutex<Option<String>>>,
    /// Signalled when a session id is first captured.
    session_set: Arc<Notify>,
    /// Last observed SSE event id.
    last_event_id: Arc<Mutex<Option<String>>>,
    /// Headers attached to every HTTP request.
    static_headers: HeaderMap,
    /// Sender half for outbound JSON-RPC messages.
    sender: Option<mpsc::UnboundedSender<JSONRPCMessage>>,
    /// Receiver half for inbound JSON-RPC messages.
    receiver: Option<mpsc::UnboundedReceiver<JSONRPCMessage>>,
    /// Cancellation token stopping background tasks.
    shutdown: CancellationToken,
    /// OAuth client for attaching bearer tokens.
    oauth_client: Option<Arc<OAuth2Client>>,
}

/// Everything a background task needs to send messages and route replies.
#[derive(Clone)]
struct OutboundContext {
    /// HTTP client used to send requests.
    client: HttpClient,
    /// Endpoint URL for HTTP transport.
    endpoint: String,
    /// Session identifier returned by the server.
    session_id: Arc<Mutex<Option<String>>>,
    /// Signalled when a session id is first captured.
    session_set: Arc<Notify>,
    /// Last observed SSE event id.
    last_event_id: Arc<Mutex<Option<String>>>,
    /// Headers attached to every HTTP request.
    static_headers: HeaderMap,
    /// OAuth client for attaching bearer tokens.
    oauth_client: Option<Arc<OAuth2Client>>,
    /// Channel delivering inbound messages to the connection loop.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
}

/// Stream wrapper for HTTP transport
struct HttpTransportStream {
    /// Sender for outgoing JSON-RPC messages.
    sender: mpsc::UnboundedSender<JSONRPCMessage>,
    /// Receiver for incoming JSON-RPC messages.
    receiver: mpsc::UnboundedReceiver<JSONRPCMessage>,
    /// Join handle for the outbound dispatch task.
    dispatch_task: JoinHandle<()>,
    /// Cancellation token stopping the SSE manager and in-flight sends.
    shutdown: CancellationToken,
}

impl Drop for HttpTransportStream {
    fn drop(&mut self) {
        self.dispatch_task.abort();
        self.shutdown.cancel();
    }
}

/// Capture a session id from response headers for initialize requests.
async fn update_session_id(is_initialize: bool, headers: &HeaderMap, ctx: &OutboundContext) {
    if !is_initialize {
        return;
    }

    let Some(sid) = headers.get("Mcp-Session-Id") else {
        return;
    };
    let Ok(sid_str) = sid.to_str() else {
        return;
    };

    let mut guard = ctx.session_id.lock().await;
    *guard = Some(sid_str.to_string());
    debug!("Got session ID: {}", sid_str);
    drop(guard);
    ctx.session_set.notify_one();
}

/// Return true if the message expects a JSON-RPC response body.
fn expects_response(msg: &JSONRPCMessage) -> bool {
    matches!(msg, JSONRPCMessage::Request(_))
}

/// Parse a JSON-RPC response and forward it over the channel.
async fn forward_response(
    msg: &JSONRPCMessage,
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
            forward_request_error(
                msg,
                INTERNAL_ERROR,
                format!("Invalid response from server: {e}"),
                sender,
            );
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
///
/// If the stream ends without delivering a response to `msg`, a synthetic
/// error is forwarded so the request fails promptly instead of timing out.
async fn forward_sse_response(
    msg: &JSONRPCMessage,
    response: reqwest::Response,
    sender: &mpsc::UnboundedSender<JSONRPCMessage>,
    last_event_id: &Arc<Mutex<Option<String>>>,
) {
    let mut saw_response = false;
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
                if let Ok(message) = serde_json::from_str::<JSONRPCMessage>(&event.data) {
                    saw_response |= matches!(message, JSONRPCMessage::Response(_));
                    if sender.unbounded_send(message).is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                error!("SSE response error: {:?}", e);
                break;
            }
        }
    }
    if !saw_response {
        forward_request_error(
            msg,
            INTERNAL_ERROR,
            "Server closed the response stream without responding".to_string(),
            sender,
        );
    }
}

/// Handle an HTTP response for a single outbound JSON-RPC message.
async fn handle_http_response(
    msg: &JSONRPCMessage,
    response: reqwest::Response,
    ctx: &OutboundContext,
) {
    let status = response.status();
    if !status.is_success() {
        error!("HTTP request failed with status: {}", status);
        forward_status_error(msg, status, &ctx.sender);
        return;
    }

    if !expects_response(msg) {
        return;
    }

    if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
        return;
    }

    if response_is_sse(&response) {
        forward_sse_response(msg, response, &ctx.sender, &ctx.last_event_id).await;
    } else {
        forward_response(msg, response, &ctx.sender).await;
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
        // No total request timeout: SSE streams and long calls are bounded by
        // the idle read timeout instead, which keep-alives reset.
        let mut client = HttpClient::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT);
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
            session_set: Arc::new(Notify::new()),
            last_event_id: Arc::new(Mutex::new(None)),
            static_headers: HeaderMap::new(),
            sender: None,
            receiver: None,
            shutdown: CancellationToken::new(),
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

/// Connect the standing SSE GET stream and forward events until it ends.
async fn connect_sse(ctx: &OutboundContext, shutdown: &CancellationToken) -> Result<SseOutcome> {
    let Some(session_id_value) = ctx.session_id.lock().await.clone() else {
        return Ok(SseOutcome::NoSession);
    };

    let mut headers = ctx.static_headers.clone();
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

    if let Some(last_event_id_value) = ctx.last_event_id.lock().await.clone() {
        headers.insert(
            "Last-Event-ID",
            HeaderValue::from_str(&last_event_id_value)
                .map_err(|_| Error::Transport("Invalid Last-Event-ID".into()))?,
        );
    }

    if let Some(oauth_client) = &ctx.oauth_client {
        let token = oauth_client.get_valid_token().await?;
        headers.insert(header::AUTHORIZATION, bearer_header(&token)?);
    }

    let response = ctx
        .client
        .get(&ctx.endpoint)
        .headers(headers)
        .send()
        .await
        .map_err(|e| Error::Transport(format!("Failed to connect SSE: {e}")))?;

    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
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
                            let mut guard = ctx.last_event_id.lock().await;
                            *guard = Some(event_id);
                        }
                        if let Ok(msg) = serde_json::from_str::<JSONRPCMessage>(&event.data)
                            && ctx.sender.unbounded_send(msg).is_err()
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

/// Keep the standing SSE stream connected with exponential backoff.
///
/// Waits for the session id to be captured, then reconnects whenever the
/// stream ends. The server replays missed events via `Last-Event-ID`.
async fn run_sse_manager(ctx: OutboundContext, shutdown: CancellationToken) {
    let mut backoff = SSE_RECONNECT_MIN;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        if ctx.session_id.lock().await.is_none() {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = ctx.session_set.notified() => continue,
            }
        }

        let connected_at = Instant::now();
        match connect_sse(&ctx, &shutdown).await {
            Ok(SseOutcome::NotSupported) => {
                debug!("Server does not support the SSE endpoint");
                return;
            }
            Ok(_) | Err(_) => {
                if shutdown.is_cancelled() {
                    return;
                }
                // A stream that stayed up for a while resets the backoff.
                if connected_at.elapsed() >= SSE_RECONNECT_MAX {
                    backoff = SSE_RECONNECT_MIN;
                }
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(SSE_RECONNECT_MAX);
            }
        }
    }
}

/// Send one outbound message, retrying once after an OAuth refresh on 401.
async fn send_outbound(ctx: OutboundContext, msg: JSONRPCMessage) {
    debug!("HTTP client sending message: {:?}", msg);

    let is_initialize =
        matches!(&msg, JSONRPCMessage::Request(req) if req.request.method == "initialize");

    let request = match outbound_post_request(&ctx).await {
        Ok(request) => request,
        Err(error) => {
            forward_prepare_error(&ctx, &msg, &error);
            return;
        }
    };

    let mut response_result = send_http_message(&ctx, request.headers, &msg).await;

    if matches!(&response_result, Ok(response) if response.status() == StatusCode::UNAUTHORIZED)
        && let Some(retried) = retry_unauthorized(&ctx, request.access_token.as_deref(), &msg).await
    {
        response_result = retried;
    }

    dispatch_response(&ctx, &msg, is_initialize, response_result).await;
}

/// Route the HTTP outcome of one outbound message back to the connection.
async fn dispatch_response(
    ctx: &OutboundContext,
    msg: &JSONRPCMessage,
    is_initialize: bool,
    response_result: reqwest::Result<reqwest::Response>,
) {
    match response_result {
        Ok(response) => {
            debug!("HTTP response status: {}", response.status());
            update_session_id(is_initialize, response.headers(), ctx).await;
            handle_http_response(msg, response, ctx).await;
        }
        Err(e) => {
            error!("Failed to send HTTP request to {}: {:?}", ctx.endpoint, e);
            forward_request_error(
                msg,
                INTERNAL_ERROR,
                format!("HTTP request failed: {e}"),
                &ctx.sender,
            );
        }
    }
}

/// Report a header-preparation failure to the waiting request, if any.
fn forward_prepare_error(ctx: &OutboundContext, msg: &JSONRPCMessage, error: &Error) {
    error!("Failed to prepare HTTP request headers: {}", error);
    let code = if matches!(error, Error::AuthorizationFailed(_)) {
        AUTHORIZATION_FAILED
    } else {
        INTERNAL_ERROR
    };
    forward_request_error(msg, code, error.to_string(), &ctx.sender);
}

/// Refresh the OAuth token after a 401 and resend the message.
///
/// Returns None when no retry is possible (no OAuth configured, the token
/// already changed, or the refresh failed).
async fn retry_unauthorized(
    ctx: &OutboundContext,
    sent_token: Option<&str>,
    msg: &JSONRPCMessage,
) -> Option<reqwest::Result<reqwest::Response>> {
    let oauth = ctx.oauth_client.as_ref()?;
    let sent_token = sent_token?;

    if let Err(error) = oauth.refresh_access_token_if_current(sent_token).await {
        error!(
            "OAuth token refresh failed after HTTP 401; re-authentication required: {}",
            error
        );
        return None;
    }
    match outbound_post_request(ctx).await {
        Ok(request) => Some(send_http_message(ctx, request.headers, msg).await),
        Err(error) => {
            error!("Failed to prepare retried HTTP request headers: {}", error);
            None
        }
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

        Ok(())
    }

    fn framed(mut self: Box<Self>) -> Result<Box<dyn TransportStream>> {
        let sender = self.sender.take().ok_or(Error::TransportDisconnected)?;
        let receiver = self.receiver.take().ok_or(Error::TransportDisconnected)?;

        let ctx = OutboundContext {
            client: self.client.clone(),
            endpoint: self.endpoint.clone(),
            session_id: self.session_id.clone(),
            session_set: self.session_set.clone(),
            last_event_id: self.last_event_id.clone(),
            static_headers: self.static_headers.clone(),
            oauth_client: self.oauth_client.clone(),
            sender,
        };

        let shutdown = self.shutdown.clone();
        tokio::spawn(run_sse_manager(ctx.clone(), shutdown));

        let (http_tx, mut http_rx) = mpsc::unbounded::<JSONRPCMessage>();
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));

        let dispatch_task = tokio::spawn(async move {
            while let Some(msg) = http_rx.next().await {
                let task_ctx = ctx.clone();
                if expects_response(&msg) {
                    let Ok(permit) = semaphore.clone().acquire_owned().await else {
                        break;
                    };
                    tokio::spawn(async move {
                        let _permit = permit;
                        send_outbound(task_ctx, msg).await;
                    });
                } else {
                    // Notifications and responses bypass the request limit so
                    // cancellations are never stuck behind slow calls.
                    tokio::spawn(send_outbound(task_ctx, msg));
                }
            }
        });

        Ok(Box::new(HttpTransportStream {
            sender: http_tx,
            receiver,
            dispatch_task,
            shutdown: self.shutdown.clone(),
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
async fn outbound_post_request(ctx: &OutboundContext) -> Result<OutboundPostRequest> {
    let mut headers = ctx.static_headers.clone();
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

    if let Some(ref sid) = *ctx.session_id.lock().await {
        headers.insert(
            "Mcp-Session-Id",
            HeaderValue::from_str(sid)
                .map_err(|_| Error::Transport("Invalid session ID".into()))?,
        );
    }

    let mut access_token = None;
    if let Some(oauth_client) = &ctx.oauth_client {
        let token = oauth_client.get_valid_token().await?;
        headers.insert(header::AUTHORIZATION, bearer_header(&token)?);
        access_token = Some(token);
    }

    Ok(OutboundPostRequest {
        headers,
        access_token,
    })
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
    ctx: &OutboundContext,
    headers: HeaderMap,
    msg: &JSONRPCMessage,
) -> reqwest::Result<reqwest::Response> {
    ctx.client
        .post(&ctx.endpoint)
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
            dispatch_task: tokio::spawn(async {}), // Dummy task for testing
            shutdown: shutdown1,
        };

        let mut stream2 = HttpTransportStream {
            sender: tx2,
            receiver: rx1,
            dispatch_task: tokio::spawn(async {}), // Dummy task for testing
            shutdown: shutdown2,
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
