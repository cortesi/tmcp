use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use futures::{SinkExt, StreamExt};
use serde::Serialize;
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
    connection::ServerHandler,
    context::ServerCtx,
    error::{Error, Result, ToolError},
    http::HttpServerTransport,
    jsonrpc::create_jsonrpc_notification,
    schema::{self, *},
    transport::{GenericDuplex, StdioTransport, StreamTransport, Transport},
};

/// MCP Server implementation
pub struct Server<F> {
    /// Factory for creating per-connection handlers.
    connection_factory: F,
}

impl Server<()> {
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
    ///             .with_tools(true)           // Enable tools capability
    ///             .with_resources(true, true) // Enable resources with subscribe and list_changed
    ///             .with_prompts(true)         // Enable prompts capability
    ///             .with_logging()             // Enable logging capability
    ///             .with_instructions("A helpful MCP server"))
    ///     }
    /// }
    ///
    /// let server = Server::new(|| MyHandler);
    /// server.serve_stdio().await?;
    /// ```
    pub fn new<C, G>(
        factory: G,
    ) -> Server<impl Fn() -> Box<dyn ServerHandler> + Clone + Send + Sync + 'static>
    where
        C: ServerHandler + 'static,
        G: Fn() -> C + Clone + Send + Sync + 'static,
    {
        Server {
            connection_factory: move || Box::new(factory()) as Box<dyn ServerHandler>,
        }
    }
}

impl<F> Server<F>
where
    F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
{
    /// Create a server from a pre-boxed handler factory.
    ///
    /// This is for internal use when the factory already returns `Box<dyn ServerHandler>`.
    pub(crate) fn from_factory(factory: F) -> Self {
        Self {
            connection_factory: factory,
        }
    }

    /// Serve a single connection using the provided transport
    /// This is a convenience method that starts the server and waits for completion
    pub(crate) async fn serve(self, transport: Box<dyn Transport>) -> Result<()> {
        let handle = ServerHandle::new(self, transport).await?;
        handle
            .handle
            .await
            .map_err(|e| Error::InternalError(format!("Server task failed: {e}")))
    }

    /// Serve connections from stdin/stdout
    /// This is a convenience method for the common stdio use case
    pub async fn serve_stdio(self) -> Result<()> {
        let transport = Box::new(StdioTransport::new());
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
    pub async fn serve_tcp(self, addr: impl ToSocketAddrs) -> Result<TcpServerHandle>
    where
        F: Clone,
    {
        let listener = TcpListener::bind(addr).await?;
        let bound_addr = listener.local_addr()?;
        info!("MCP server listening on {}", bound_addr);

        // Convert connection factory to Arc for sharing across tasks
        let connection_factory = Arc::new(self.connection_factory);

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

                                // Clone Arc reference for the spawned task
                                let factory = connection_factory.clone();

                                // Handle each connection in a separate task
                                tokio::spawn(async move {
                                    // Create a new server with cloned factory
                                    let server = Self {
                                        connection_factory: factory.as_ref().clone(),
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

    /// Serve HTTP connections
    /// This is a convenience method for the common HTTP server use case
    /// Returns a ServerHandle that can be used to stop the server
    pub async fn serve_http(self, addr: impl AsRef<str>) -> Result<ServerHandle> {
        let mut http_transport = HttpServerTransport::new(addr.as_ref());
        http_transport.start().await?;
        let bound_addr = http_transport.bind_addr.clone();

        let mut handle = ServerHandle::from_transport(self, Box::new(http_transport)).await?;
        handle.bound_addr = Some(bound_addr);
        Ok(handle)
    }
}

/// Handle for controlling a running MCP server instance
pub struct ServerHandle {
    /// Join handle for the server task.
    pub handle: JoinHandle<()>,
    /// Sender for outbound server notifications.
    notification_tx: mpsc::UnboundedSender<ServerNotification>,
    /// Token used to signal shutdown to the server loop.
    shutdown_token: CancellationToken,
    /// The actual bound address (for servers that bind to a network port)
    pub bound_addr: Option<String>,
    /// Capabilities from the handler's initialize response.
    /// This is set when the client initializes and is used to gate notifications.
    capabilities: Arc<RwLock<ServerCapabilities>>,
}

impl ServerHandle {
    /// Start serving connections using the provided transport, returning a handle for runtime operations
    pub(crate) async fn new<F>(server: Server<F>, mut transport: Box<dyn Transport>) -> Result<Self>
    where
        F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
    {
        transport.connect().await?;
        let remote_addr = transport.remote_addr();
        let stream = transport.framed()?;
        let (sink_tx, mut stream_rx) = stream.split();

        info!("MCP server started");
        let (notification_tx, mut notification_rx) = mpsc::unbounded_channel();

        // Channel for queueing responses to be sent
        let (response_tx, mut response_rx) = mpsc::unbounded_channel::<JSONRPCMessage>();

        // Wrap the sink in an Arc<Mutex> for sharing
        let sink_tx = Arc::new(Mutex::new(sink_tx));

        // Clone notification_tx for the handle
        let notification_tx_handle = notification_tx.clone();

        // Create connection instance wrapped in Arc for shared access
        let connection = Arc::new((server.connection_factory)());

        // Create a single ServerCtx instance that will be used throughout the connection
        let server_ctx = ServerCtx::new(notification_tx, Some(sink_tx.clone()));

        // Create shutdown token for coordinating shutdown
        let shutdown_token = CancellationToken::new();
        let shutdown_token_task = shutdown_token.clone();

        // Shared capabilities - updated when client initializes
        let capabilities = Arc::new(RwLock::new(ServerCapabilities::default()));
        let capabilities_task = capabilities.clone();

        // Track whether we've called on_connect after initialization
        let mut initialized = false;

        // Start the main server loop in a background task
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Check for shutdown signal
                    _ = shutdown_token_task.cancelled() => {
                        info!("Server received shutdown signal");
                        break;
                    }
                    // Handle incoming messages from client
                    result = stream_rx.next() => {
                        match result {
                            Some(Ok(message)) => {
                                match message {
                                    JSONRPCMessage::Request(request)
                                        if !initialized && request.request.method == "initialize" =>
                                    {
                                        // Handle initialize specially to capture capabilities
                                        let (response, init_caps) =
                                            handle_initialize_request(
                                                connection.as_ref().as_ref(),
                                                request,
                                                &server_ctx
                                            ).await;

                                        let should_connect = init_caps.is_some();

                                        // Store capabilities from the handler's response
                                        if let Some(caps) = init_caps
                                            && let Ok(mut guard) = capabilities_task.write() {
                                                *guard = caps;
                                            }

                                        {
                                            let mut sink = sink_tx.lock().await;
                                            if let Err(e) = sink.send(response).await {
                                                error!("Error sending initialize response: {}", e);
                                                break;
                                            }
                                        }

                                        if should_connect {
                                            if let Err(e) = connection.on_connect(&server_ctx, &remote_addr).await {
                                                error!("Error during on_connect: {}", e);
                                                break;
                                            }
                                            initialized = true;
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
                                        server_ctx.handle_client_response(response).await;
                                    }
                                    other => {
                                        if let Err(e) = handle_message_with_connection(
                                            connection.clone(),
                                            other,
                                            response_tx.clone(),
                                            &server_ctx,
                                        )
                                        .await
                                        {
                                            error!("Error handling message: {}", e);
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                error!("Error reading message: {}", e);
                                break;
                            }
                            None => {
                                info!("Client disconnected");
                                break;
                            }
                        }
                    }

                    // Forward internal notifications to client
                    Some(notification) = notification_rx.recv() => {
                        let jsonrpc_notification = create_jsonrpc_notification(&notification);
                        {
                            let mut sink = sink_tx.lock().await;
                            if let Err(e) = sink.send(JSONRPCMessage::Notification(jsonrpc_notification)).await {
                                error!("Error sending notification to client: {}", e);
                                break;
                            }
                        }
                    }

                    // Send queued responses to client
                    Some(response) = response_rx.recv() => {
                        let mut sink = sink_tx.lock().await;
                        if let Err(e) = sink.send(response).await {
                            error!("Error sending response to client: {}", e);
                            break;
                        }
                    }
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
            capabilities,
        })
    }

    /// Create a ServerHandle using generic AsyncRead and AsyncWrite streams
    /// This is a convenience method that creates a StreamTransport from the provided streams
    pub async fn from_stream<F, R, W>(server: Server<F>, reader: R, writer: W) -> Result<Self>
    where
        F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
        R: AsyncRead + Send + Sync + Unpin + 'static,
        W: AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let duplex = GenericDuplex::new(reader, writer);
        let transport = Box::new(StreamTransport::new(duplex));
        Self::new(server, transport).await
    }

    /// Create a ServerHandle from a transport
    /// This allows using any transport implementation
    pub async fn from_transport<F>(server: Server<F>, transport: Box<dyn Transport>) -> Result<Self>
    where
        F: Fn() -> Box<dyn ServerHandler> + Send + Sync + 'static,
    {
        Self::new(server, transport).await
    }

    /// Stop the server and wait for the background task to finish.
    pub async fn stop(self) -> Result<()> {
        // Signal shutdown
        self.shutdown_token.cancel();

        // Wait for the server task to complete
        self.handle
            .await
            .map_err(|e| Error::InternalError(format!("Server task failed: {e}")))?;
        Ok(())
    }

    /// Send a server notification to connected clients.
    pub fn send_server_notification(&self, notification: &ServerNotification) {
        if !self.can_forward_notification(notification) {
            debug!(
                "Skipping server notification {:?} due to missing capability",
                notification
            );
            return;
        }
        if let Err(e) = self.notification_tx.send(notification.clone()) {
            error!(
                "Failed to send server notification {:?}: {}",
                notification, e
            );
        }
    }

    /// Check whether a notification is supported by the configured capabilities.
    fn can_forward_notification(&self, notification: &ServerNotification) -> bool {
        let caps = self.capabilities.read().unwrap_or_else(|e| e.into_inner());
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
    /// The actual bound address.
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

/// Handle a message using the Connection trait
async fn handle_message_with_connection(
    connection: Arc<Box<dyn ServerHandler>>,
    message: JSONRPCMessage,
    response_tx: mpsc::UnboundedSender<JSONRPCMessage>,
    context: &ServerCtx,
) -> Result<()> {
    if let JSONRPCMessage::Notification(notification) = message {
        handle_notification(&**connection, notification, context).await?;
        return Ok(());
    }

    handle_message_without_await(&connection, message, response_tx, context);
    Ok(())
}

/// Handle messages that do not require awaiting on the connection.
#[allow(clippy::cognitive_complexity)]
fn handle_message_without_await(
    connection: &Arc<Box<dyn ServerHandler>>,
    message: JSONRPCMessage,
    response_tx: mpsc::UnboundedSender<JSONRPCMessage>,
    context: &ServerCtx,
) {
    if let JSONRPCMessage::Request(request) = message {
        spawn_request_handler(connection, request, response_tx, context);
        return;
    }

    if let JSONRPCMessage::Response(_) = message {
        // Response handling is done in the main message loop
        debug!("Response handling delegated to main loop");
    }
}

/// Spawn a task to handle a request and send its response.
fn spawn_request_handler(
    connection: &Arc<Box<dyn ServerHandler>>,
    request: JSONRPCRequest,
    response_tx: mpsc::UnboundedSender<JSONRPCMessage>,
    context: &ServerCtx,
) {
    let conn = connection.clone();
    let ctx = context.clone();
    let tx = response_tx;

    tokio::spawn(async move {
        let response_message = handle_request(&**conn, request.clone(), &ctx).await;
        tracing::info!("Server sending response: {:?}", response_message);

        if let Err(e) = tx.send(response_message) {
            error!("Failed to queue response: {}", e);
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
    match connection
        .initialize(
            &ctx_with_request,
            protocol_version,
            capabilities,
            client_info,
        )
        .await
    {
        Ok(result) => {
            // Capture the capabilities from the result
            let caps = result.capabilities.clone();

            // Serialize the result
            match serde_json::to_value(&result) {
                Ok(value) => {
                    let response =
                        JSONRPCMessage::Response(JSONRPCResponse::Result(JSONRPCResultResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: request.id,
                            result: schema::JSONRpcResult {
                                _meta: None,
                                other: if let Some(obj) = value.as_object() {
                                    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                                } else {
                                    HashMap::new()
                                },
                            },
                        }));
                    (response, Some(caps))
                }
                Err(e) => (
                    JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: Some(request.id),
                        error: ErrorObject {
                            code: INTERNAL_ERROR,
                            message: format!("Failed to serialize initialize result: {e}"),
                            data: None,
                        },
                    })),
                    None,
                ),
            }
        }
        Err(e) => {
            let response = if let Some(jsonrpc_error) = e.to_jsonrpc_response(request.id.clone()) {
                JSONRPCMessage::Response(JSONRPCResponse::Error(jsonrpc_error))
            } else {
                JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id),
                    error: ErrorObject {
                        code: INTERNAL_ERROR,
                        message: e.to_string(),
                        data: None,
                    },
                }))
            };
            (response, None)
        }
    }
}

/// Handle a request using the Connection trait and convert result to JSONRPCMessage
async fn handle_request(
    connection: &dyn ServerHandler,
    request: JSONRPCRequest,
    context: &ServerCtx,
) -> JSONRPCMessage {
    tracing::info!(
        "Server handling request: {:?} method: {}",
        request.id,
        request.request.method
    );
    // Create a context with the request ID
    let ctx_with_request = context.with_request_id(request.id.clone());
    let result = handle_request_inner(connection, request.clone(), &ctx_with_request).await;

    match result {
        Ok(value) => {
            // Create a successful response
            JSONRPCMessage::Response(JSONRPCResponse::Result(JSONRPCResultResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: request.id,
                result: schema::JSONRpcResult {
                    _meta: None,
                    other: if let Some(obj) = value.as_object() {
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                    } else {
                        let mut map = HashMap::new();
                        map.insert("result".to_string(), value);
                        map
                    },
                },
            }))
        }
        Err(e) => {
            // Check if error has a specific JSONRPC response
            if let Some(jsonrpc_error) = e.to_jsonrpc_response(request.id.clone()) {
                JSONRPCMessage::Response(JSONRPCResponse::Error(jsonrpc_error))
            } else {
                // For all other errors, use INTERNAL_ERROR
                JSONRPCMessage::Response(JSONRPCResponse::Error(JSONRPCErrorResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(request.id),
                    error: ErrorObject {
                        code: INTERNAL_ERROR,
                        message: e.to_string(),
                        data: None,
                    },
                }))
            }
        }
    }
}

/// Inner handler that returns Result<serde_json::Value>
async fn handle_request_inner(
    conn: &dyn ServerHandler,
    request: JSONRPCRequest,
    ctx: &ServerCtx,
) -> Result<serde_json::Value> {
    let JSONRPCRequest {
        request: Request { method, params },
        ..
    } = request;
    let client_request = parse_client_request(method, params)?;

    dispatch_client_request(conn, ctx, client_request).await
}

/// Parse a client request from the JSON-RPC method and params payload.
fn parse_client_request(method: String, params: Option<RequestParams>) -> Result<ClientRequest> {
    let mut request_obj = serde_json::Map::new();
    request_obj.insert(
        "method".to_string(),
        serde_json::Value::String(method.clone()),
    );
    if let Some(params) = params {
        if let Some(meta) = params._meta {
            request_obj.insert("_meta".to_string(), serde_json::to_value(meta)?);
        }
        for (key, value) in params.other {
            request_obj.insert(key, value);
        }
    }

    match serde_json::from_value::<ClientRequest>(serde_json::Value::Object(request_obj)) {
        Ok(req) => Ok(req),
        Err(err) => {
            let err_str = err.to_string();
            if err_str.contains("unknown variant") {
                Err(Error::MethodNotFound(method))
            } else {
                Err(Error::InvalidParams(format!(
                    "Invalid parameters for {}: {}",
                    method, err
                )))
            }
        }
    }
}

/// Dispatch a parsed client request to the appropriate handler.
async fn dispatch_client_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::Initialize {
            protocol_version,
            capabilities,
            client_info,
            _meta: _,
        } => serialize_result(
            conn.initialize(ctx, protocol_version, *capabilities, client_info)
                .await,
        ),
        ClientRequest::Ping { .. } => {
            info!("Server received ping request, sending automatic response");
            empty_result(conn.pong(ctx).await)
        }
        ClientRequest::ListTools { .. } | ClientRequest::CallTool { .. } => {
            handle_tool_request(conn, ctx, request).await
        }
        ClientRequest::ListResources { .. }
        | ClientRequest::ListResourceTemplates { .. }
        | ClientRequest::ReadResource { .. }
        | ClientRequest::Subscribe { .. }
        | ClientRequest::Unsubscribe { .. } => handle_resource_request(conn, ctx, request).await,
        ClientRequest::ListPrompts { .. } | ClientRequest::GetPrompt { .. } => {
            handle_prompt_request(conn, ctx, request).await
        }
        ClientRequest::Complete { .. } => handle_completion_request(conn, ctx, request).await,
        ClientRequest::SetLevel { .. } => handle_logging_request(conn, ctx, request).await,
        ClientRequest::GetTask { .. }
        | ClientRequest::GetTaskPayload { .. }
        | ClientRequest::ListTasks { .. }
        | ClientRequest::CancelTask { .. } => handle_task_request(conn, ctx, request).await,
    }
}

/// Handle tool-related client requests.
async fn handle_tool_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::ListTools { cursor, _meta: _ } => {
            serialize_result(conn.list_tools(ctx, cursor).await)
        }
        ClientRequest::CallTool {
            name,
            arguments,
            task,
            _meta: _,
        } => {
            let result = conn.call_tool(ctx, name, arguments, task).await;
            match result {
                Ok(result) => serialize_result(Ok(result)),
                Err(Error::InvalidParams(message)) => {
                    let tool_result: CallToolResult = ToolError::invalid_input(message).into();
                    serialize_result(Ok(tool_result))
                }
                Err(err) => Err(err),
            }
        }
        _ => Err(Error::InternalError("Unexpected tool request".to_string())),
    }
}

/// Handle resource-related client requests.
async fn handle_resource_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::ListResources { cursor, _meta: _ } => {
            serialize_result(conn.list_resources(ctx, cursor).await)
        }
        ClientRequest::ListResourceTemplates { cursor, _meta: _ } => {
            serialize_result(conn.list_resource_templates(ctx, cursor).await)
        }
        ClientRequest::ReadResource { uri, _meta: _ } => {
            serialize_result(conn.read_resource(ctx, uri).await)
        }
        ClientRequest::Subscribe { uri, _meta: _ } => {
            empty_result(conn.resources_subscribe(ctx, uri).await)
        }
        ClientRequest::Unsubscribe { uri, _meta: _ } => {
            empty_result(conn.resources_unsubscribe(ctx, uri).await)
        }
        _ => Err(Error::InternalError(
            "Unexpected resource request".to_string(),
        )),
    }
}

/// Handle prompt-related client requests.
async fn handle_prompt_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::ListPrompts { cursor, _meta: _ } => {
            serialize_result(conn.list_prompts(ctx, cursor).await)
        }
        ClientRequest::GetPrompt {
            name,
            arguments,
            _meta: _,
        } => serialize_result(conn.get_prompt(ctx, name, arguments).await),
        _ => Err(Error::InternalError(
            "Unexpected prompt request".to_string(),
        )),
    }
}

/// Handle completion-related client requests.
async fn handle_completion_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::Complete {
            reference,
            argument,
            context,
            _meta: _,
        } => serialize_result(conn.complete(ctx, reference, argument, context).await),
        _ => Err(Error::InternalError(
            "Unexpected completion request".to_string(),
        )),
    }
}

/// Handle logging-related client requests.
async fn handle_logging_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::SetLevel { level, _meta: _ } => {
            empty_result(conn.set_level(ctx, level).await)
        }
        _ => Err(Error::InternalError(
            "Unexpected logging request".to_string(),
        )),
    }
}

/// Handle task-related client requests.
async fn handle_task_request(
    conn: &dyn ServerHandler,
    ctx: &ServerCtx,
    request: ClientRequest,
) -> Result<serde_json::Value> {
    match request {
        ClientRequest::GetTask { task_id, _meta: _ } => {
            serialize_result(conn.get_task(ctx, task_id).await)
        }
        ClientRequest::GetTaskPayload { task_id, _meta: _ } => {
            serialize_result(conn.get_task_payload(ctx, task_id).await)
        }
        ClientRequest::ListTasks { cursor, _meta: _ } => {
            serialize_result(conn.list_tasks(ctx, cursor).await)
        }
        ClientRequest::CancelTask { task_id, _meta: _ } => {
            serialize_result(conn.cancel_task(ctx, task_id).await)
        }
        _ => Err(Error::InternalError("Unexpected task request".to_string())),
    }
}

/// Serialize a handler result into JSON for a JSON-RPC response.
fn serialize_result<T: Serialize>(result: Result<T>) -> Result<serde_json::Value> {
    result.and_then(|value| serde_json::to_value(value).map_err(Into::into))
}

/// Convert a unit result into an empty JSON object response.
fn empty_result(result: Result<()>) -> Result<serde_json::Value> {
    result.map(|_| serde_json::json!({}))
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

    // Build a value that matches the shape expected by ClientNotification.
    let mut object = serde_json::Map::new();
    object.insert(
        "method".to_string(),
        serde_json::Value::String(notification.notification.method.clone()),
    );

    if let Some(params) = notification.notification.params {
        if let Some(meta) = params._meta {
            object.insert("_meta".to_string(), serde_json::to_value(meta)?);
        }
        for (k, v) in params.other {
            object.insert(k, v);
        }
    }

    let value = serde_json::Value::Object(object);

    match serde_json::from_value::<ClientNotification>(value) {
        Ok(typed) => connection.notification(context, typed).await,
        Err(e) => {
            warn!("Failed to deserialize client notification: {}", e);
            Ok(())
        }
    }
}
