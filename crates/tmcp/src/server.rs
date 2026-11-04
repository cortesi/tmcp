#[cfg(unix)]
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use std::{
    net::SocketAddr,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, ToSocketAddrs},
    runtime::Builder,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::{
    Error, Result,
    connection::ServerHandler,
    context::ServerCtx,
    jsonrpc::{
        create_jsonrpc_notification, jsonrpc_response, parse_typed_notification,
        parse_typed_request, result_to_jsonrpc_response,
    },
    request_handler::RequestHandler,
    schema::*,
    transport::{GenericDuplex, StdioTransport, StreamTransport, Transport},
};

/// Fan-out delivering a server notification to every active session.
pub type NotificationFanout = Box<dyn Fn(&ServerNotification) + Send + Sync>;

/// Factory invoked once per connection to create its handler.
pub type HandlerFactory = Arc<dyn Fn() -> Box<dyn ServerHandler> + Send + Sync>;

/// Maximum number of queued outbound server notifications before backpressure
/// applies.
const SERVER_NOTIFICATION_BUFFER: usize = 64;
/// Maximum number of queued server responses before request handlers
/// backpressure.
const SERVER_RESPONSE_BUFFER: usize = 64;

/// MCP Server implementation
pub struct Server {
    /// Factory for creating per-connection handlers.
    connection_factory: HandlerFactory,
    /// MCP protocol versions in server preference order.
    protocol_versions: SupportedProtocolVersions,
}

impl Server {
    /// Create a new server with a handler factory.
    ///
    /// The factory function is called once for each incoming connection,
    /// allowing each connection to have its own handler instance with
    /// independent state.
    ///
    /// Server capabilities are specified by returning them from the handler's
    /// [`ServerHandler::initialize`] method. This makes the handler the single
    /// source of truth for what the server advertises to clients.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tmcp::{Server, ServerHandler, ServerCtx, Result};
    /// use tmcp::schema::{ClientCapabilities, Implementation, InitializeResult};
    ///
    /// struct MyHandler;
    ///
    /// #[async_trait::async_trait]
    /// impl ServerHandler for MyHandler {
    ///     async fn initialize(
    ///         &self,
    ///         _ctx: &ServerCtx,
    ///         _protocol_version: ProtocolVersion,
    ///         _capabilities: ClientCapabilities,
    ///         _client_info: Implementation,
    ///     ) -> Result<InitializeResult> {
    ///         // Specify server capabilities here
    ///         Ok(InitializeResult::new("my-server")
    ///             .with_version("1.0.0")
    ///             .with_tools(None)           // Enable static tools capability
    ///             .with_resources(Some(true), Some(true)) // Enable resources with subscribe and list_changed
    ///             .with_prompts(Some(true))   // Enable prompts capability
    ///             .with_logging()             // Enable logging capability
    ///             .with_instructions("A helpful MCP server"))
    ///     }
    /// }
    ///
    /// let server = Server::new(|| MyHandler);
    /// server.serve_stdio().await?;
    /// ```
    pub fn new<C, G>(factory: G) -> Self
    where
        C: ServerHandler + 'static,
        G: Fn() -> C + Send + Sync + 'static,
    {
        Self {
            connection_factory: Arc::new(move || Box::new(factory()) as Box<dyn ServerHandler>),
            protocol_versions: SupportedProtocolVersions::default(),
        }
    }

    /// Replace the MCP protocol versions accepted by this server.
    pub fn with_protocol_versions(mut self, versions: SupportedProtocolVersions) -> Self {
        self.protocol_versions = versions;
        self
    }

    /// Create a server from a shared, pre-boxed handler factory.
    ///
    /// A hook for the HTTP transport of `tmcp-http`, which creates one server
    /// for each session. Not a stable API.
    #[doc(hidden)]
    pub fn from_factory(
        factory: HandlerFactory,
        protocol_versions: SupportedProtocolVersions,
    ) -> Self {
        Self {
            connection_factory: factory,
            protocol_versions,
        }
    }

    /// Return the handler factory and the protocol versions of the server.
    ///
    /// A hook for the HTTP transport of `tmcp-http`. Not a stable API.
    #[doc(hidden)]
    pub fn into_factory(self) -> (HandlerFactory, SupportedProtocolVersions) {
        (self.connection_factory, self.protocol_versions)
    }

    /// Serve a single connection using the provided transport
    /// This is a convenience method that starts the server and waits for
    /// completion
    pub(crate) async fn serve(self, transport: Box<dyn Transport>) -> Result<()> {
        let handle = ServerHandle::new(self, transport).await?;
        handle.join().await
    }

    /// Serve connections from stdin/stdout.
    ///
    /// This is a convenience method for the common stdio use case.
    ///
    /// `stdout` is reserved for JSON-RPC traffic while this server is running.
    /// Do not print human logs to `stdout` or install a tracing/logging
    /// subscriber that writes there; route diagnostics to `stderr`, a file,
    /// or another sink instead.
    pub async fn serve_stdio(self) -> Result<()> {
        let transport = Box::new(StdioTransport);
        self.serve(transport).await
    }

    /// Serve connections from stdin/stdout using an internal Tokio runtime.
    ///
    /// This is a convenience for binaries that aren't already running within a
    /// Tokio runtime.
    pub fn serve_stdio_blocking(self) -> Result<()> {
        let rt = Builder::new_multi_thread().enable_all().build()?;
        rt.block_on(self.serve_stdio())
    }

    /// Serve using generic AsyncRead and AsyncWrite streams
    /// This is a convenience method that creates a StreamTransport from the
    /// provided streams
    pub async fn serve_stream<R, W>(self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Send + Sync + Unpin + 'static,
        W: AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let duplex = GenericDuplex::new(reader, writer);
        let transport = Box::new(StreamTransport::new(duplex));
        self.serve(transport).await
    }

    /// Serve TCP connections by accepting them in a loop
    ///
    /// Returns a [`TcpServerHandle`] that can be used to stop accepting new
    /// connections. Existing connections will continue until they complete
    /// or their clients disconnect.
    pub async fn serve_tcp(self, addr: impl ToSocketAddrs) -> Result<TcpServerHandle> {
        let listener = TcpListener::bind(addr).await?;
        let bound_addr = listener.local_addr()?;
        info!("MCP server listening on {}", bound_addr);

        // The shared connection factory is cloned into each connection task.
        let connection_factory = self.connection_factory;
        let protocol_versions = self.protocol_versions;

        // Create shutdown token for coordinating shutdown
        let shutdown_token = CancellationToken::new();
        let shutdown_token_loop = shutdown_token.clone();

        // Spawn the accept loop
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Check for shutdown signal
                    _ = shutdown_token_loop.cancelled() => {
                        info!("TCP server shutting down");
                        break;
                    }
                    // Accept new connections
                    result = listener.accept() => {
                        match result {
                            Ok((stream, peer_addr)) => {
                                info!("New connection from {}", peer_addr);

                                Self::spawn_connection(
                                    connection_factory.clone(),
                                    protocol_versions.clone(),
                                    stream,
                                    peer_addr.to_string(),
                                );
                            }
                            Err(e) => {
                                error!("Failed to accept connection: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Ok(TcpServerHandle {
            handle,
            shutdown_token,
            bound_addr,
        })
    }

    /// Serve Unix domain socket connections at `path` by accepting them in a
    /// loop.
    ///
    /// The server binds a new socket file at `path`, which must not exist, and
    /// gives it mode 0600, so that only its owner can connect. A socket in a
    /// directory that only the owner can enter closes the window between the
    /// bind and the mode change. The returned [`UnixServerHandle`] stops the
    /// accept loop and removes the socket file.
    #[cfg(unix)]
    pub async fn serve_unix(self, path: impl AsRef<Path>) -> Result<UnixServerHandle> {
        let path = path.as_ref().to_path_buf();
        let listener = UnixListener::bind(&path)?;
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            drop(listener);
            remove_socket_file(&path)?;
            return Err(error.into());
        }
        info!("MCP server listening on {}", path.display());

        let connection_factory = self.connection_factory;
        let protocol_versions = self.protocol_versions;
        let shutdown_token = CancellationToken::new();
        let shutdown_token_loop = shutdown_token.clone();
        let socket_name = path.display().to_string();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_token_loop.cancelled() => {
                        info!("Unix socket server shutting down");
                        break;
                    }
                    result = listener.accept() => {
                        match result {
                            Ok((stream, _peer)) => {
                                info!("New connection on {}", socket_name);
                                Self::spawn_connection(
                                    connection_factory.clone(),
                                    protocol_versions.clone(),
                                    stream,
                                    socket_name.clone(),
                                );
                            }
                            Err(e) => {
                                error!("Failed to accept connection: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Ok(UnixServerHandle {
            handle,
            shutdown_token,
            path,
        })
    }

    /// Serve one accepted connection in a task of its own.
    fn spawn_connection<S>(
        connection_factory: HandlerFactory,
        protocol_versions: SupportedProtocolVersions,
        stream: S,
        peer: String,
    ) where
        S: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    {
        tokio::spawn(async move {
            let server = Self {
                connection_factory,
                protocol_versions,
            };
            let transport = Box::new(StreamTransport::new(stream));
            match server.serve(transport).await {
                Ok(()) => info!("Connection from {} closed", peer),
                Err(e) => error!("Error handling connection from {}: {}", peer, e),
            }
        });
    }
}

/// Remove a socket file, tolerating one that is already gone.
#[cfg(unix)]
fn remove_socket_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Handle for controlling a running MCP server instance
pub struct ServerHandle {
    /// Join handle for the server task.
    handle: JoinHandle<()>,
    /// Sender for outbound server notifications.
    notification_tx: mpsc::Sender<ServerNotification>,
    /// Token used to signal shutdown to the server loop.
    shutdown_token: CancellationToken,
    /// The bound listener address, when the server is bound to a network port.
    ///
    /// `None` for stdio and stream transports, which have no socket address.
    pub bound_addr: Option<String>,
    /// Public HTTP endpoint path, when the server is exposed over HTTP.
    endpoint_path: Option<String>,
    /// Notification fan-out override used by multi-session (HTTP) listeners.
    ///
    /// When set, `send_server_notification` delegates here instead of the
    /// single-connection notification channel.
    fanout: Option<NotificationFanout>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // An abandoned handle must not leak its connection loop.
        self.shutdown_token.cancel();
    }
}

impl ServerHandle {
    /// Start serving connections using the provided transport, returning a
    /// handle for runtime operations
    pub(crate) async fn new(server: Server, mut transport: Box<dyn Transport>) -> Result<Self> {
        transport.connect().await?;
        let remote_addr = transport.remote_addr();
        let stream = transport.framed()?;
        let (sink_tx, mut stream_rx) = stream.split();

        info!("MCP server started");
        let (notification_tx, mut notification_rx) = mpsc::channel(SERVER_NOTIFICATION_BUFFER);

        // Single ordered queue for loop-transmitted traffic: request responses
        // (None marks a suppressed response) and gated notifications, in FIFO
        // order so a notification emitted before a response is sent before it.
        let (outbound_tx, mut outbound_rx) =
            mpsc::channel::<Option<JSONRPCMessage>>(SERVER_RESPONSE_BUFFER);

        // Wrap the sink in an Arc<Mutex> for sharing
        let sink_tx = Arc::new(Mutex::new(sink_tx));

        // Control queue for the cancellation notices of abandoned custom
        // requests. It shares no capacity with the bounded queues, so a full
        // queue cannot drop a notice.
        let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel::<RequestId>();

        // Clone notification_tx for the handle
        let notification_tx_handle = notification_tx.clone();

        // Create connection instance wrapped in Arc for shared access
        let connection: Arc<dyn ServerHandler> = Arc::from((server.connection_factory)());
        let protocol_versions = server.protocol_versions;

        // Create a single ServerCtx instance that will be used throughout the
        // connection
        let server_ctx = ServerCtx::connected(notification_tx, sink_tx.clone(), cancel_tx);

        // Create shutdown token for coordinating shutdown
        let shutdown_token = CancellationToken::new();
        let shutdown_token_task = shutdown_token.clone();

        // Negotiated capabilities - captured from the handler's initialize
        // response and used to gate notifications.
        let capabilities = Arc::new(RwLock::new(ServerCapabilities::default()));

        // Track whether we've called on_connect after initialization
        let mut initialized = false;
        let mut client_disconnected = false;
        let in_flight_requests = Arc::new(AtomicUsize::new(0));

        // Start the main server loop in a background task
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Check for shutdown signal
                    _ = shutdown_token_task.cancelled() => {
                        info!("Server received shutdown signal");
                        server_ctx.shutdown_requests();
                        break;
                    }
                    // Handle incoming messages from client
                    result = stream_rx.next(), if !client_disconnected => {
                        match result {
                            Some(Ok(incoming)) => {
                                let context = server_ctx.with_extensions(incoming.extensions);
                                let message = incoming.message;
                                match message {
                                    JSONRPCMessage::Request(request)
                                        if request.request.method == "initialize" =>
                                    {
                                        let response = if initialized {
                                            // Re-initialization is a protocol error.
                                            reinitialize_error_response(request.id)
                                        } else {
                                            // Handle initialize specially to capture capabilities
                                            let (response, init_caps) =
                                                handle_initialize_request(
                                                    connection.as_ref(),
                                                    request,
                                                    &context,
                                                    &protocol_versions,
                                                ).await;

                                            // Store capabilities from the handler's response
                                            if let Some(caps) = init_caps {
                                                {
                                                    let mut guard = capabilities
                                                        .write()
                                                        .unwrap_or_else(|e| e.into_inner());
                                                    *guard = caps;
                                                }

                                                if let Err(e) = connection.on_connect(&context, &remote_addr).await {
                                                    error!("Error during on_connect: {}", e);
                                                    break;
                                                }
                                                initialized = true;
                                            }
                                            response
                                        };

                                        let mut sink = sink_tx.lock().await;
                                        if let Err(e) = sink.send(response).await {
                                            error!("Error sending initialize response: {}", e);
                                            break;
                                        }
                                    }
                                    JSONRPCMessage::Response(response) => {
                                        let response_id = match &response {
                                            JSONRPCResponse::Result(result) => {
                                                Some(result.id.clone())
                                            }
                                            JSONRPCResponse::Error(error) => error.id.clone(),
                                        };
                                        tracing::info!(
                                            "Server received response from client: {:?}",
                                            response_id
                                        );
                                        context.handle_client_response(response).await;
                                    }
                                    other => {
                                        if let Err(e) = handle_message_with_connection(
                                            connection.clone(),
                                            other,
                                            outbound_tx.clone(),
                                            in_flight_requests.clone(),
                                            &context,
                                        )
                                        .await
                                        {
                                            error!("Error handling message: {}", e);
                                        }
                                    }
                                }
                            }
                            // A malformed line was consumed by the codec; answer
                            // with a JSON-RPC parse error and keep the connection.
                            Some(Err(Error::JsonParse { message })) => {
                                warn!("Malformed JSON-RPC message: {}", message);
                                let mut sink = sink_tx.lock().await;
                                if let Err(e) = sink.send(parse_error_response()).await {
                                    error!("Error sending parse error response: {}", e);
                                    server_ctx.shutdown_requests();
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                error!("Error reading message: {}", e);
                                server_ctx.shutdown_requests();
                                break;
                            }
                            None => {
                                info!("Client disconnected");
                                server_ctx.shutdown_requests();
                                client_disconnected = true;
                            }
                        }
                    }

                    // Gate and queue internal notifications behind any earlier
                    // responses, preserving emission order on the wire.
                    Some(notification) = notification_rx.recv() => {
                        let permitted = {
                            let caps = capabilities.read().unwrap_or_else(|e| e.into_inner());
                            notification_permitted(&caps, &notification)
                        };
                        if permitted {
                            let message = JSONRPCMessage::Notification(
                                create_jsonrpc_notification(&notification),
                            );
                            if outbound_tx.try_send(Some(message)).is_err() {
                                debug!("Outbound queue full; dropping notification");
                            }
                        } else {
                            debug!(
                                "Skipping notification {:?} due to missing capability",
                                notification
                            );
                        }
                    }

                    // Transmit queued outbound traffic in order. None marks a
                    // suppressed (cancelled) response and only wakes the loop.
                    Some(item) = outbound_rx.recv() => {
                        if let Some(message) = item {
                            let mut sink = sink_tx.lock().await;
                            if let Err(e) = sink.send(message).await {
                                error!("Error sending response to client: {}", e);
                                server_ctx.shutdown_requests();
                                break;
                            }
                        }
                    }

                    // Tell the client about each custom request that the
                    // server abandoned.
                    Some(request_id) = cancel_rx.recv() => {
                        let mut sink = sink_tx.lock().await;
                        if let Err(e) = sink.send(cancellation_notice(request_id)).await {
                            error!("Error sending cancellation to client: {}", e);
                            server_ctx.shutdown_requests();
                            break;
                        }
                    }
                }

                if client_disconnected
                    && in_flight_requests.load(Ordering::SeqCst) == 0
                    && outbound_rx.is_empty()
                    && cancel_rx.is_empty()
                {
                    break;
                }
            }

            // Cover every exit, including initialization and write failures.
            server_ctx.shutdown_requests();

            // Clean up connection
            if let Err(e) = connection.on_shutdown().await {
                error!("Error during server shutdown: {}", e);
            }

            info!("MCP server stopped");
        });

        Ok(Self {
            handle,
            notification_tx: notification_tx_handle,
            shutdown_token,
            bound_addr: None,
            endpoint_path: None,
            fanout: None,
        })
    }

    /// Create a handle for a multi-session listener (HTTP).
    ///
    /// `task` keeps the listener alive for `join`; `shutdown_token` stops the
    /// listener and all of its sessions; `fanout` delivers server
    /// notifications to every active session.
    ///
    /// A hook for the HTTP transport of `tmcp-http`. Not a stable API.
    #[doc(hidden)]
    pub fn listener(
        task: JoinHandle<()>,
        shutdown_token: CancellationToken,
        fanout: NotificationFanout,
    ) -> Self {
        // The notification channel is unused when fanout is set; sessions own
        // their own channels.
        let (notification_tx, _notification_rx) = mpsc::channel(1);
        Self {
            handle: task,
            notification_tx,
            shutdown_token,
            bound_addr: None,
            endpoint_path: None,
            fanout: Some(fanout),
        }
    }

    /// Signal the server loop to stop without consuming the handle.
    ///
    /// A hook for the HTTP transport of `tmcp-http`. Not a stable API.
    #[doc(hidden)]
    pub fn signal_stop(&self) {
        self.shutdown_token.cancel();
    }

    /// Record the HTTP endpoint of a listener: its bound address, when it
    /// listens itself, and its normalized endpoint path.
    ///
    /// A hook for the HTTP transport of `tmcp-http`. Not a stable API.
    #[doc(hidden)]
    pub fn set_http_endpoint(&mut self, bound_addr: Option<String>, endpoint_path: String) {
        if bound_addr.is_some() {
            self.bound_addr = bound_addr;
        }
        self.endpoint_path = Some(endpoint_path);
    }

    /// Create a ServerHandle using generic AsyncRead and AsyncWrite streams
    /// This is a convenience method that creates a StreamTransport from the
    /// provided streams
    pub async fn from_stream<R, W>(server: Server, reader: R, writer: W) -> Result<Self>
    where
        R: AsyncRead + Send + Sync + Unpin + 'static,
        W: AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let duplex = GenericDuplex::new(reader, writer);
        let transport = Box::new(StreamTransport::new(duplex));
        Self::new(server, transport).await
    }

    /// Create a ServerHandle from a transport
    /// This allows using any transport implementation
    pub async fn from_transport(server: Server, transport: Box<dyn Transport>) -> Result<Self> {
        Self::new(server, transport).await
    }

    /// Stop the server and wait for the background task to finish.
    pub async fn stop(self) -> Result<()> {
        // Signal shutdown
        self.shutdown_token.cancel();

        self.join().await
    }

    /// Wait for the server task to finish without signaling shutdown.
    pub async fn join(mut self) -> Result<()> {
        (&mut self.handle)
            .await
            .map_err(|e| Error::InternalError(format!("Server task failed: {e}")))?;
        Ok(())
    }

    /// Return the externally reachable endpoint address, including any HTTP
    /// path.
    #[must_use]
    pub fn endpoint_addr(&self) -> Option<String> {
        let bound_addr = self.bound_addr.as_deref()?;
        match self.endpoint_path.as_deref() {
            None | Some("/") => Some(bound_addr.to_string()),
            Some(endpoint_path) => Some(format!("{bound_addr}{endpoint_path}")),
        }
    }

    /// Send a server notification to connected clients.
    ///
    /// Notifications are gated against each connection's negotiated
    /// capabilities by its connection loop.
    pub fn send_server_notification(&self, notification: &ServerNotification) {
        if let Some(fanout) = &self.fanout {
            fanout(notification);
            return;
        }
        if let Err(e) = self.notification_tx.try_send(notification.clone()) {
            error!(
                "Failed to send server notification {:?}: {}",
                notification, e
            );
        }
    }
}

/// Handle for controlling a running TCP MCP server
///
/// Unlike [`ServerHandle`] which manages a single connection, `TcpServerHandle`
/// manages an accept loop that spawns handlers for multiple connections.
pub struct TcpServerHandle {
    /// Join handle for the accept loop task.
    handle: JoinHandle<()>,
    /// Token used to signal shutdown to the accept loop.
    shutdown_token: CancellationToken,
    /// The bound listener address.
    ///
    /// Always present: TCP servers are always bound to a socket address.
    pub bound_addr: SocketAddr,
}

/// Handle for controlling a running Unix domain socket MCP server
///
/// Like [`TcpServerHandle`], it manages an accept loop that spawns handlers
/// for multiple connections.
#[cfg(unix)]
pub struct UnixServerHandle {
    /// Join handle for the accept loop task.
    handle: JoinHandle<()>,
    /// Token used to signal shutdown to the accept loop.
    shutdown_token: CancellationToken,
    /// The path of the bound socket file.
    pub path: PathBuf,
}

#[cfg(unix)]
impl UnixServerHandle {
    /// Stop accepting new connections, wait for the accept loop to finish, and
    /// remove the socket file.
    ///
    /// Existing connections continue until they complete or their clients
    /// disconnect.
    pub async fn stop(self) -> Result<()> {
        self.shutdown_token.cancel();
        self.handle
            .await
            .map_err(|e| Error::InternalError(format!("Unix socket accept loop failed: {e}")))?;
        remove_socket_file(&self.path)?;
        Ok(())
    }
}

impl TcpServerHandle {
    /// Stop accepting new connections and wait for the accept loop to finish.
    ///
    /// Note: This stops accepting new connections but does not terminate
    /// existing connections - they will continue until they complete or
    /// their clients disconnect.
    pub async fn stop(self) -> Result<()> {
        // Signal shutdown
        self.shutdown_token.cancel();

        // Wait for the accept loop to complete
        self.handle
            .await
            .map_err(|e| Error::InternalError(format!("TCP accept loop failed: {e}")))?;
        Ok(())
    }
}

/// Build the notification that cancels an abandoned request of the server.
fn cancellation_notice(request_id: RequestId) -> JSONRPCMessage {
    let notification = ServerNotification::cancelled(
        Some(request_id),
        Some("the server stopped waiting for the response".to_owned()),
    );
    JSONRPCMessage::Notification(create_jsonrpc_notification(&notification))
}

/// Build the error response for a repeated initialize request.
fn reinitialize_error_response(id: RequestId) -> JSONRPCMessage {
    JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(id),
        error: ErrorObject {
            code: INVALID_REQUEST,
            message: "initialize may only be sent once per connection".to_string(),
            data: None,
        },
    }))
}

/// Build the JSON-RPC parse error response for a malformed message.
fn parse_error_response() -> JSONRPCMessage {
    JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        error: ErrorObject {
            code: PARSE_ERROR,
            message: "Parse error".to_string(),
            data: None,
        },
    }))
}

/// Check whether a notification is permitted by the negotiated capabilities.
fn notification_permitted(caps: &ServerCapabilities, notification: &ServerNotification) -> bool {
    match notification {
        ServerNotification::LoggingMessage { .. } => caps.logging.is_some(),
        ServerNotification::ResourceUpdated { .. } => caps
            .resources
            .as_ref()
            .and_then(|c| c.subscribe)
            .unwrap_or(false),
        ServerNotification::ResourceListChanged { .. } => caps
            .resources
            .as_ref()
            .and_then(|c| c.list_changed)
            .unwrap_or(false),
        ServerNotification::ToolListChanged { .. } => caps
            .tools
            .as_ref()
            .and_then(|c| c.list_changed)
            .unwrap_or(false),
        ServerNotification::PromptListChanged { .. } => caps
            .prompts
            .as_ref()
            .and_then(|c| c.list_changed)
            .unwrap_or(false),
        ServerNotification::ElicitationComplete { .. } => true,
        ServerNotification::TaskStatus { .. } => caps.tasks.is_some(),
        ServerNotification::Progress { .. } | ServerNotification::Cancelled { .. } => true,
    }
}

/// Handle a message using the Connection trait
async fn handle_message_with_connection(
    connection: Arc<dyn ServerHandler>,
    message: JSONRPCMessage,
    outbound_tx: mpsc::Sender<Option<JSONRPCMessage>>,
    in_flight_requests: Arc<AtomicUsize>,
    context: &ServerCtx,
) -> Result<()> {
    match message {
        JSONRPCMessage::Notification(notification) => {
            handle_notification(&*connection, notification, context).await
        }
        JSONRPCMessage::Request(request) => {
            in_flight_requests.fetch_add(1, Ordering::SeqCst);
            spawn_request_handler(
                &connection,
                request,
                outbound_tx,
                in_flight_requests,
                context,
            );
            Ok(())
        }
        JSONRPCMessage::Response(_) => {
            // Response handling is done in the main message loop
            debug!("Response handling delegated to main loop");
            Ok(())
        }
    }
}

/// Spawn a task to handle a request and queue its outcome.
///
/// Every request queues exactly one item: its response, or None when the
/// response is suppressed by cancellation. The None still wakes the
/// connection loop so post-disconnect draining always terminates.
fn spawn_request_handler(
    connection: &Arc<dyn ServerHandler>,
    request: JSONRPCRequest,
    outbound_tx: mpsc::Sender<Option<JSONRPCMessage>>,
    in_flight_requests: Arc<AtomicUsize>,
    context: &ServerCtx,
) {
    let conn = connection.clone();

    // Track the request as in flight before yielding to the loop, so a
    // cancellation notification processed next is not ignored.
    let ctx = context.begin_request(&request.id);

    tokio::spawn(async move {
        let request_id = request.id.clone();
        let response = dispatch_request(&*conn, request, &ctx).await;
        let suppressed = ctx.finish_request();
        in_flight_requests.fetch_sub(1, Ordering::SeqCst);

        let payload = if suppressed {
            tracing::info!("Server suppressing response for cancelled request: {request_id:?}");
            None
        } else {
            let message = JSONRPCMessage::Response(response);
            tracing::info!("Server sending {}", message.summary());
            Some(message)
        };
        if outbound_tx.send(payload).await.is_err() {
            error!("Failed to queue response for request {request_id:?}");
        }
    });
}

/// Handle an initialize request specially, returning both the response and
/// capabilities.
///
/// This is needed so the `ServerHandle` can capture the capabilities from the
/// handler's response rather than from a separate configuration.
async fn handle_initialize_request(
    connection: &dyn ServerHandler,
    request: JSONRPCRequest,
    context: &ServerCtx,
    protocol_versions: &SupportedProtocolVersions,
) -> (JSONRPCMessage, Option<ServerCapabilities>) {
    let JSONRPCRequest {
        id,
        request: Request { params, .. },
        ..
    } = request;
    let ctx_with_request = context.with_request_id(id.clone());

    let initialize_request = match parse_typed_request::<ClientRequest>("initialize", params) {
        Ok(initialize_request) => initialize_request,
        Err(error) => {
            return (
                result_to_jsonrpc_response::<InitializeResult>(id, Err(error)),
                None,
            );
        }
    };
    let ClientRequest::Initialize {
        protocol_version,
        capabilities,
        client_info,
        ..
    } = initialize_request
    else {
        unreachable!("initialize method deserialized as another request")
    };
    let result = initialize(
        connection,
        &ctx_with_request,
        protocol_version,
        *capabilities,
        client_info,
        protocol_versions,
    )
    .await;
    let caps = result
        .as_ref()
        .ok()
        .map(|result| result.capabilities.clone());
    (result_to_jsonrpc_response(id, result), caps)
}

/// Negotiate the protocol version of an initialize request and call the
/// initialize method of the handler.
async fn initialize<H: ServerHandler + ?Sized>(
    connection: &H,
    context: &ServerCtx,
    requested_version: ProtocolVersion,
    capabilities: ClientCapabilities,
    client_info: Implementation,
    protocol_versions: &SupportedProtocolVersions,
) -> Result<InitializeResult> {
    let protocol_version = if protocol_versions.contains(&requested_version) {
        requested_version
    } else {
        protocol_versions.latest().clone()
    };
    let mut result = connection
        .initialize(context, protocol_version.clone(), capabilities, client_info)
        .await?;
    result.protocol_version = protocol_version;
    Ok(result)
}

/// Parse one request, dispatch it to the handler with the request context
/// `context`, and build its response.
async fn dispatch_request(
    connection: &dyn ServerHandler,
    request: JSONRPCRequest,
    context: &ServerCtx,
) -> JSONRPCResponse {
    let JSONRPCRequest {
        id,
        request: Request { method, params },
        ..
    } = request;
    tracing::info!("Server handling request: {:?} method: {}", id, method);

    let result = match parse_typed_request::<ClientRequest>(&method, params) {
        Ok(client_request) => connection.handle_request(context, client_request).await,
        Err(e) => Err(e),
    };
    jsonrpc_response(id, result)
}

/// Handle one client request with a server handler, without a transport.
///
/// The request takes the path of a request on a connection: tmcp calls
/// [`ServerHandler::handle_request`], so an override of it (middleware) and
/// the conversion of invalid tool parameters into a tool error apply. The
/// result and the error are those that a [`Client`](crate::Client) across a
/// transport returns, and the result decodes into `T`.
///
/// `context` is the context of this request: derive it with
/// [`ServerCtx::for_request`] from a connection context, such as
/// [`ServerCtx::in_process`]. An initialize request negotiates the protocol
/// version as a connection does. The host calls
/// [`ServerHandler::on_connect`], [`ServerHandler::notification`], and
/// [`ServerHandler::on_shutdown`] itself.
///
/// The handler runs in the task of the caller. When the token of `context` is
/// cancelled, the handler sees the cancellation, and this function returns
/// [`Error::Cancelled`] after the handler finishes, because a connection
/// sends no response for a cancelled request.
///
/// # Errors
///
/// Returns [`Error::InvalidRequest`] when `context` has no request ID.
pub async fn handle_request<H, T>(
    handler: &H,
    context: &ServerCtx,
    request: ClientRequest,
) -> Result<T>
where
    H: ServerHandler + ?Sized,
    T: DeserializeOwned,
{
    let Some(request_id) = context.request_id().cloned() else {
        return Err(Error::InvalidRequest(
            "a request context needs a request ID: use ServerCtx::for_request".to_owned(),
        ));
    };
    let method = request.method();
    let result = match request {
        ClientRequest::Initialize {
            protocol_version,
            capabilities,
            client_info,
            ..
        } => initialize(
            handler,
            context,
            protocol_version,
            *capabilities,
            client_info,
            &SupportedProtocolVersions::default(),
        )
        .await
        .and_then(|result| serde_json::to_value(result).map_err(Into::into)),
        request => handler.handle_request(context, request).await,
    };
    if context.is_cancelled() {
        return Err(Error::Cancelled {
            request_id: request_id.to_string(),
        });
    }
    RequestHandler::decode_response(method, jsonrpc_response(request_id, result))
}

/// Handle a notification using the Connection trait
async fn handle_notification(
    connection: &dyn ServerHandler,
    notification: JSONRPCNotification,
    context: &ServerCtx,
) -> Result<()> {
    debug!(
        "Received notification: {}",
        notification.notification.method
    );

    match parse_typed_notification::<ClientNotification>(notification.notification) {
        Ok(typed) => {
            if let ClientNotification::Cancelled {
                request_id,
                reason: _,
                _meta: _,
            } = &typed
                && let Some(request_id) = request_id.clone()
            {
                context.mark_cancelled(&request_id);
            }
            connection.notification(context, typed).await
        }
        Err(e) => {
            warn!("Failed to deserialize client notification: {}", e);
            Ok(())
        }
    }
}
