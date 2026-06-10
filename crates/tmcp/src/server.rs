use std::{
    net::SocketAddr,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

#[cfg(feature = "http")]
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, ToSocketAddrs},
    runtime::Builder,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

#[cfg(feature = "auth")]
use crate::auth::server::{AuthConfig, BearerAuthLayer, protected_resource_handler};
#[cfg(feature = "http")]
use crate::http::{CorsPolicy, EmbeddedHttpRoutes, HttpServer, normalize_endpoint_path};
use crate::{
    connection::ServerHandler,
    context::ServerCtx,
    error::{Error, Result},
    jsonrpc::{
        create_jsonrpc_notification, parse_typed_notification, parse_typed_request,
        result_to_jsonrpc_response,
    },
    schema::*,
    transport::{GenericDuplex, StdioTransport, StreamTransport, Transport},
};

/// Fan-out delivering a server notification to every active session.
pub type NotificationFanout = Box<dyn Fn(&ServerNotification) + Send + Sync>;

/// Factory invoked once per connection to create its handler.
pub type HandlerFactory = Arc<dyn Fn() -> Box<dyn ServerHandler> + Send + Sync>;

/// Maximum number of queued outbound server notifications before backpressure applies.
const SERVER_NOTIFICATION_BUFFER: usize = 64;
/// Maximum number of queued server responses before request handlers backpressure.
const SERVER_RESPONSE_BUFFER: usize = 64;

/// Builder that configures and serves the HTTP transport.
#[cfg(feature = "http")]
pub struct HttpBuilder {
    /// Server to serve.
    server: Server,
    /// Bind address for the HTTP listener.
    bind_addr: Option<String>,
    /// Public endpoint path where MCP is served.
    endpoint_path: String,
    /// Transformation applied to the MCP routes after state is attached.
    middleware: Option<Box<dyn FnOnce(Router) -> Router + Send>>,
    /// Additional routes merged outside the middleware scope.
    routes: Option<Router>,
    /// Cross-origin policy for the MCP routes.
    cors: CorsPolicy,
}

/// Result of embedding tmcp HTTP routes into an existing Axum application.
#[cfg(feature = "http")]
pub struct EmbeddedHttpServer {
    /// Router containing the mounted MCP handlers and any auxiliary routes.
    ///
    /// Merge this router into the host application at the root.
    pub router: Router,
    /// Live tmcp server handle that must be shut down with the host application.
    pub handle: ServerHandle,
}

/// MCP Server implementation
pub struct Server {
    /// Factory for creating per-connection handlers.
    connection_factory: HandlerFactory,
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
    ///         _protocol_version: String,
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
        }
    }

    /// Create a server from a shared, pre-boxed handler factory.
    #[cfg(feature = "http")]
    pub(crate) fn from_factory(factory: HandlerFactory) -> Self {
        Self {
            connection_factory: factory,
        }
    }

    /// Serve a single connection using the provided transport
    /// This is a convenience method that starts the server and waits for completion
    pub(crate) async fn serve(self, transport: Box<dyn Transport>) -> Result<()> {
        let handle = ServerHandle::new(self, transport).await?;
        handle.join().await
    }

    /// Serve connections from stdin/stdout.
    ///
    /// This is a convenience method for the common stdio use case.
    ///
    /// `stdout` is reserved for JSON-RPC traffic while this server is running. Do not print human
    /// logs to `stdout` or install a tracing/logging subscriber that writes there; route
    /// diagnostics to `stderr`, a file, or another sink instead.
    pub async fn serve_stdio(self) -> Result<()> {
        let transport = Box::new(StdioTransport);
        self.serve(transport).await
    }

    /// Serve connections from stdin/stdout using an internal Tokio runtime.
    ///
    /// This is a convenience for binaries that aren't already running within a Tokio runtime.
    pub fn serve_stdio_blocking(self) -> Result<()> {
        let rt = Builder::new_multi_thread().enable_all().build()?;
        rt.block_on(self.serve_stdio())
    }

    /// Serve using generic AsyncRead and AsyncWrite streams
    /// This is a convenience method that creates a StreamTransport from the provided streams
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
    /// Returns a [`TcpServerHandle`] that can be used to stop accepting new connections.
    /// Existing connections will continue until they complete or their clients disconnect.
    pub async fn serve_tcp(self, addr: impl ToSocketAddrs) -> Result<TcpServerHandle> {
        let listener = TcpListener::bind(addr).await?;
        let bound_addr = listener.local_addr()?;
        info!("MCP server listening on {}", bound_addr);

        // The shared connection factory is cloned into each connection task.
        let connection_factory = self.connection_factory;

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

                                // Clone the shared factory for the spawned task
                                let factory = connection_factory.clone();

                                // Handle each connection in a separate task
                                tokio::spawn(async move {
                                    // Create a new server with the shared factory
                                    let server = Self {
                                        connection_factory: factory,
                                    };

                                    let transport = Box::new(StreamTransport::new(stream));

                                    match server.serve(transport).await {
                                        Ok(()) => info!("Connection from {} closed", peer_addr),
                                        Err(e) => error!("Error handling connection from {}: {}", peer_addr, e),
                                    }
                                });
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

    /// Configure an HTTP server bound to the provided address.
    ///
    /// Returns an [`HttpBuilder`] for further configuration; call
    /// [`HttpBuilder::serve`] to start serving.
    #[cfg(feature = "http")]
    pub fn http(self, addr: impl AsRef<str>) -> HttpBuilder {
        HttpBuilder {
            server: self,
            bind_addr: Some(addr.as_ref().to_string()),
            endpoint_path: "/".to_string(),
            middleware: None,
            routes: None,
            cors: CorsPolicy::default(),
        }
    }

    /// Configure an HTTP server for embedding into an existing Axum application.
    #[cfg(feature = "http")]
    pub fn http_embed(self) -> HttpBuilder {
        HttpBuilder {
            server: self,
            bind_addr: None,
            endpoint_path: "/".to_string(),
            middleware: None,
            routes: None,
            cors: CorsPolicy::default(),
        }
    }

    /// Serve HTTP connections
    /// This is a convenience method for the common HTTP server use case
    /// Returns a ServerHandle that can be used to stop the server
    #[cfg(feature = "http")]
    pub async fn serve_http(self, addr: impl AsRef<str>) -> Result<ServerHandle> {
        self.http(addr).serve().await
    }
}

#[cfg(feature = "http")]
impl HttpBuilder {
    /// Override the public endpoint path where the MCP handlers are mounted.
    pub fn with_endpoint_path(mut self, endpoint_path: impl Into<String>) -> Self {
        self.endpoint_path = normalize_endpoint_path(endpoint_path);
        self
    }

    /// Wrap the MCP routes in middleware after state has been attached.
    pub fn with_middleware<G>(mut self, middleware: G) -> Self
    where
        G: FnOnce(Router) -> Router + Send + 'static,
    {
        self.middleware = Some(match self.middleware.take() {
            Some(previous) => Box::new(move |router| middleware(previous(router))),
            None => Box::new(middleware),
        });
        self
    }

    /// Merge additional routes that bypass the configured middleware.
    pub fn with_routes(mut self, routes: Router) -> Self {
        self.routes = Some(match self.routes.take() {
            Some(existing) => existing.merge(routes),
            None => routes,
        });
        self
    }

    /// Set the cross-origin policy for the MCP routes.
    ///
    /// The default is [`CorsPolicy::SameOrigin`], which rejects cross-origin
    /// browser requests and emits no CORS headers.
    pub fn with_cors(mut self, cors: CorsPolicy) -> Self {
        self.cors = cors;
        self
    }

    /// Protect the MCP routes with bearer-token auth and expose PRM discovery routes.
    #[cfg(feature = "auth")]
    pub fn with_auth(self, config: &AuthConfig) -> Self {
        let middleware = BearerAuthLayer::new(config.validator.clone(), &config.endpoint_path)
            .with_base_url(&config.base_url)
            .with_trusted_forwarded_headers(config.trust_forwarded_headers);
        self.with_endpoint_path(&config.endpoint_path)
            .with_middleware(|router| router.layer(middleware))
            .with_routes(protected_resource_handler(config))
    }

    /// Start serving the configured HTTP transport.
    pub async fn serve(self) -> Result<ServerHandle> {
        let bind_addr = self.bind_addr.clone().ok_or_else(|| {
            Error::InvalidConfiguration(
                "HTTP embed builders do not have a bind address; call into_router()".to_string(),
            )
        })?;
        let http = HttpServer::new(self.server.connection_factory, self.cors);
        let EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        } = http.routes(&self.endpoint_path, self.middleware, self.routes);
        let router = mcp_router.merge(aux_routes);

        let (task, bound_addr) = http.listen(&bind_addr, router).await?;
        let mut handle =
            ServerHandle::listener(task, http.shutdown_token(), http.notification_fanout());
        handle.bound_addr = Some(bound_addr);
        handle.endpoint_path = Some(self.endpoint_path);
        Ok(handle)
    }

    /// Build tmcp HTTP routers for embedding into an existing Axum application.
    pub async fn into_router(self) -> Result<EmbeddedHttpServer> {
        let endpoint_path = self.endpoint_path;
        let http = HttpServer::new(self.server.connection_factory, self.cors);
        let EmbeddedHttpRoutes {
            mcp_router,
            aux_routes,
        } = http.routes("/", self.middleware, self.routes);
        let router = mount_embedded_router(&endpoint_path, mcp_router, aux_routes);

        let shutdown_token = http.shutdown_token();
        let watch_token = shutdown_token.clone();
        let task = tokio::spawn(async move {
            watch_token.cancelled().await;
        });
        let mut handle = ServerHandle::listener(task, shutdown_token, http.notification_fanout());
        handle.endpoint_path = Some(endpoint_path);
        Ok(EmbeddedHttpServer { router, handle })
    }
}

/// Return the externally reachable HTTP endpoint address.
#[cfg(feature = "http")]
fn endpoint_addr(bound_addr: &str, endpoint_path: &str) -> String {
    let endpoint_path = normalize_endpoint_path(endpoint_path);
    if endpoint_path == "/" {
        bound_addr.to_string()
    } else {
        format!("{bound_addr}{endpoint_path}")
    }
}

/// Return an embedded router mounted at the configured endpoint path.
#[cfg(feature = "http")]
fn mount_embedded_router(endpoint_path: &str, mcp_router: Router, aux_routes: Router) -> Router {
    let endpoint_path = normalize_endpoint_path(endpoint_path);
    if endpoint_path == "/" {
        mcp_router.merge(aux_routes)
    } else {
        Router::new()
            .nest(&endpoint_path, mcp_router)
            .merge(aux_routes)
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
    #[cfg(feature = "http")]
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
    /// Start serving connections using the provided transport, returning a handle for runtime operations
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

        // Clone notification_tx for the handle
        let notification_tx_handle = notification_tx.clone();

        // Create connection instance wrapped in Arc for shared access
        let connection: Arc<dyn ServerHandler> = Arc::from((server.connection_factory)());

        // Create a single ServerCtx instance that will be used throughout the connection
        let server_ctx = ServerCtx::new(notification_tx, Some(sink_tx.clone()));

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
                                                    &context
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
                }

                if client_disconnected
                    && in_flight_requests.load(Ordering::SeqCst) == 0
                    && outbound_rx.is_empty()
                {
                    break;
                }
            }

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
            #[cfg(feature = "http")]
            endpoint_path: None,
            fanout: None,
        })
    }

    /// Create a handle for a multi-session listener (HTTP).
    ///
    /// `task` keeps the listener alive for `join`; `shutdown_token` stops the
    /// listener and all of its sessions; `fanout` delivers server
    /// notifications to every active session.
    #[cfg(feature = "http")]
    pub(crate) fn listener(
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
    #[cfg(feature = "http")]
    pub(crate) fn signal_stop(&self) {
        self.shutdown_token.cancel();
    }

    /// Create a ServerHandle using generic AsyncRead and AsyncWrite streams
    /// This is a convenience method that creates a StreamTransport from the provided streams
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

    /// Return the externally reachable endpoint address, including any HTTP path.
    #[cfg(feature = "http")]
    #[must_use]
    pub fn endpoint_addr(&self) -> Option<String> {
        let bound_addr = self.bound_addr.as_deref()?;
        let endpoint_path = self.endpoint_path.as_deref().unwrap_or("/");
        Some(endpoint_addr(bound_addr, endpoint_path))
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
    let ctx = context.clone();

    // Track the request as in flight before yielding to the loop, so a
    // cancellation notification processed next is not ignored.
    ctx.begin_request(&request.id);

    tokio::spawn(async move {
        let request_id = request.id.clone();
        let response_message = handle_request(&*conn, request, &ctx).await;
        let suppressed = ctx.is_request_cancelled(&request_id);
        ctx.end_request(&request_id);
        in_flight_requests.fetch_sub(1, Ordering::SeqCst);

        let payload = if suppressed {
            tracing::info!("Server suppressing response for cancelled request: {request_id:?}");
            None
        } else {
            tracing::info!("Server sending response: {:?}", response_message);
            Some(response_message)
        };
        if outbound_tx.send(payload).await.is_err() {
            error!("Failed to queue response for request {request_id:?}");
        }
    });
}

/// Handle an initialize request specially, returning both the response and capabilities.
///
/// This is needed so the `ServerHandle` can capture the capabilities from the handler's
/// response rather than from a separate configuration.
async fn handle_initialize_request(
    connection: &dyn ServerHandler,
    request: JSONRPCRequest,
    context: &ServerCtx,
) -> (JSONRPCMessage, Option<ServerCapabilities>) {
    let ctx_with_request = context.with_request_id(request.id.clone());

    // Parse the initialize parameters
    let params = match &request.request.params {
        Some(params) => params,
        None => {
            return (
                JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id),
                    error: ErrorObject {
                        code: INVALID_PARAMS,
                        message: "Missing initialize parameters".to_string(),
                        data: None,
                    },
                })),
                None,
            );
        }
    };

    // Extract initialize parameters
    let protocol_version = params
        .other
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let capabilities: ClientCapabilities = params
        .other
        .get("capabilities")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let client_info: Implementation = params
        .other
        .get("clientInfo")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| Implementation::new("unknown", "0.0.0"));

    // Call the handler's initialize method
    let (caps, result) = match connection
        .initialize(
            &ctx_with_request,
            protocol_version,
            capabilities,
            client_info,
        )
        .await
    {
        Ok(result) => (Some(result.capabilities.clone()), Ok(result)),
        Err(e) => (None, Err(e)),
    };
    (result_to_jsonrpc_response(request.id, result), caps)
}

/// Handle a request using the Connection trait and convert result to JSONRPCMessage
async fn handle_request(
    connection: &dyn ServerHandler,
    request: JSONRPCRequest,
    context: &ServerCtx,
) -> JSONRPCMessage {
    let JSONRPCRequest {
        id,
        request: Request { method, params },
        ..
    } = request;
    tracing::info!("Server handling request: {:?} method: {}", id, method);

    let ctx_with_request = context.with_request_id(id.clone());
    let result = match parse_typed_request::<ClientRequest>(&method, params) {
        Ok(client_request) => {
            connection
                .handle_request(&ctx_with_request, client_request)
                .await
        }
        Err(e) => Err(e),
    };
    result_to_jsonrpc_response(id, result)
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
