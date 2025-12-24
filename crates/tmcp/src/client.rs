use std::{process::Stdio, sync::Arc};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{Child, Command},
    sync::{Mutex, broadcast},
};
use tracing::{debug, error, info, warn};

use crate::{
    api::ServerAPI,
    auth::OAuth2Client,
    connection::ClientHandler,
    context::ClientCtx,
    error::{Error, Result},
    http::HttpClientTransport,
    jsonrpc::{create_jsonrpc_notification, result_to_jsonrpc_response},
    request_handler::RequestHandler,
    schema::*,
    transport::{
        GenericDuplex, StdioTransport, StreamTransport, TcpClientTransport, Transport,
        TransportStream,
    },
};

/// Default no-op implementation of ClientHandler for unit type
#[async_trait]
impl ClientHandler for () {
    // All methods use default implementations
}

/// MCP Client implementation
pub struct Client<C = ()>
where
    C: ClientHandler + Send,
{
    /// Tracks requests and routes responses.
    request_handler: RequestHandler,
    /// Connection callbacks for server-initiated requests.
    connection: C,
    /// Context used for connection callbacks.
    context: Option<ClientCtx>,
    /// Client name reported during initialization.
    name: String,
    /// Client version reported during initialization.
    version: String,
    /// Capabilities advertised to the server.
    client_capabilities: ClientCapabilities,
    /// Tracks whether on_connect has been invoked for this connection.
    on_connect_called: bool,
}

/// Result of connecting to an MCP server via a spawned process.
pub struct ProcessConnection {
    /// Spawned child process handle.
    pub child: Child,
    /// Initialization handshake result.
    pub init: InitializeResult,
}

impl Client<()> {
    /// Create a new MCP client with default configuration.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            request_handler: RequestHandler::new(None, "req".to_string()),
            connection: (),
            context: None,
            name: name.into(),
            version: version.into(),
            client_capabilities: ClientCapabilities::default(),
            on_connect_called: false,
        }
    }

    /// Set a custom handler for server-initiated requests.
    ///
    /// The handler receives callbacks for server requests like `ping`,
    /// `create_message`, and `list_roots`.
    pub fn with_handler<C: ClientHandler>(self, handler: C) -> Client<C> {
        Client {
            request_handler: self.request_handler,
            connection: handler,
            context: self.context,
            name: self.name,
            version: self.version,
            client_capabilities: self.client_capabilities,
            on_connect_called: self.on_connect_called,
        }
    }

    /// Set the client capabilities.
    pub fn with_capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.client_capabilities = capabilities;
        self
    }
}

impl<C> Client<C>
where
    C: ClientHandler + Send + 'static,
{
    /// Connect using the provided transport
    pub(crate) async fn connect(&mut self, mut transport: Box<dyn Transport>) -> Result<()> {
        self.context = None;
        self.on_connect_called = false;
        transport.connect().await?;
        let stream = transport.framed()?;

        // Start the message handler task before storing transport
        self.start_message_handler(stream).await?;

        info!("MCP client connected");
        Ok(())
    }

    /// Initialize the connection with the server
    ///
    /// This is a convenience method that uses the client's configured name, version,
    /// and capabilities with the latest protocol version.
    ///
    /// Calling `init` triggers the `ClientHandler::on_connect` callback after the
    /// initialization handshake completes.
    pub async fn init(&mut self) -> Result<InitializeResult> {
        let client_info = Implementation::new(self.name.clone(), self.version.clone());

        <Self as ServerAPI>::initialize(
            self,
            LATEST_PROTOCOL_VERSION.to_string(),
            self.client_capabilities.clone(),
            client_info,
        )
        .await
    }

    /// Connect to a TCP server and initialize the connection
    ///
    /// This is a convenience method that creates a TCP transport,
    /// connects to the server, and performs the initialization handshake.
    ///
    /// # Arguments
    /// * `addr` - Server address in the format "host:port" (e.g., "localhost:3000", "127.0.0.1:8080")
    pub async fn connect_tcp(&mut self, addr: impl Into<String>) -> Result<InitializeResult> {
        let transport = Box::new(TcpClientTransport::new(addr));
        self.connect(transport).await?;
        self.init().await
    }

    /// Connect via stdio and initialize the connection
    ///
    /// This is a convenience method that creates a stdio transport,
    /// connects to the server, and performs the initialization handshake.
    pub async fn connect_stdio(&mut self) -> Result<InitializeResult> {
        let transport = Box::new(StdioTransport::new());
        self.connect(transport).await?;
        self.init().await
    }

    /// Connect via HTTP/HTTPS and initialize the connection
    ///
    /// This is a convenience method that creates an HTTP transport,
    /// connects to the server, and performs the initialization handshake.
    /// Both HTTP and HTTPS protocols are supported.
    ///
    /// # Arguments
    /// * `endpoint` - Server URL including protocol and path (e.g., "http://localhost:3000", "https://api.example.com/mcp")
    pub async fn connect_http(&mut self, endpoint: impl Into<String>) -> Result<InitializeResult> {
        let transport = Box::new(HttpClientTransport::new(endpoint));
        self.connect(transport).await?;
        self.init().await
    }

    /// Connect via HTTP/HTTPS with OAuth authentication and initialize the connection
    ///
    /// This method creates an HTTP transport with OAuth authentication support.
    /// The OAuth client should be pre-configured with valid tokens or ready to
    /// perform the OAuth flow.
    ///
    /// # Arguments
    /// * `endpoint` - Server URL including protocol and path
    /// * `oauth_client` - Pre-configured OAuth2Client instance
    pub async fn connect_http_with_oauth(
        &mut self,
        endpoint: impl Into<String>,
        oauth_client: Arc<OAuth2Client>,
    ) -> Result<InitializeResult> {
        let transport = Box::new(HttpClientTransport::new(endpoint).with_oauth(oauth_client));
        self.connect(transport).await?;
        self.init().await
    }

    /// Connect using generic AsyncRead and AsyncWrite streams and initialize the connection.
    ///
    /// This method allows you to connect to a server using any pair of
    /// AsyncRead and AsyncWrite streams, such as process stdio, pipes,
    /// or custom implementations.
    ///
    /// Use [`connect_stream_raw`](Self::connect_stream_raw) if you need to
    /// control initialization manually.
    pub async fn connect_stream<R, W>(&mut self, reader: R, writer: W) -> Result<InitializeResult>
    where
        R: AsyncRead + Send + Sync + Unpin + 'static,
        W: AsyncWrite + Send + Sync + Unpin + 'static,
    {
        self.connect_stream_raw(reader, writer).await?;
        self.init().await
    }

    /// Connect using generic AsyncRead and AsyncWrite streams without initialization.
    ///
    /// This method establishes a transport connection but does not perform the MCP
    /// initialization handshake.
    pub async fn connect_stream_raw<R, W>(&mut self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Send + Sync + Unpin + 'static,
        W: AsyncWrite + Send + Sync + Unpin + 'static,
    {
        let duplex = GenericDuplex::new(reader, writer);
        let transport = Box::new(StreamTransport::new(duplex));
        self.connect(transport).await
    }

    /// Spawn a process, connect to it, and initialize the MCP handshake.
    ///
    /// This method spawns a new process, establishes an MCP connection
    /// through its standard input and output streams, and runs the initialize
    /// handshake.
    ///
    /// # Arguments
    /// * `command` - A configured tokio::process::Command ready to be spawned
    ///
    /// # Returns
    /// Returns the spawned Child process handle and the initialization result.
    pub async fn connect_process(&mut self, command: Command) -> Result<ProcessConnection> {
        let mut child = self.connect_process_raw(command).await?;
        match self.init().await {
            Ok(init) => Ok(ProcessConnection { child, init }),
            Err(err) => {
                if let Err(kill_err) = child.kill().await {
                    warn!("Failed to kill process after init error: {}", kill_err);
                }
                Err(err)
            }
        }
    }

    /// Spawn a process and connect to it via its stdin/stdout without initialization.
    ///
    /// This method spawns a new process and establishes an MCP connection
    /// through its standard input and output streams.
    ///
    /// # Arguments
    /// * `command` - A configured tokio::process::Command ready to be spawned
    ///
    /// # Returns
    /// Returns the spawned Child process handle, allowing you to manage the
    /// process lifecycle (e.g., wait for completion, kill it, etc.)
    pub async fn connect_process_raw(&mut self, mut command: Command) -> Result<Child> {
        // Configure the command to use piped stdio
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()); // Let stderr pass through for debugging

        // Spawn the process
        let mut child = command
            .spawn()
            .map_err(|e| Error::Transport(format!("Failed to spawn process: {e}")))?;

        // Take ownership of stdin and stdout
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Transport("Failed to capture process stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Transport("Failed to capture process stdout".to_string()))?;

        // Connect using the process streams
        // Note: We swap the order here - the child's stdout is our reader,
        // and the child's stdin is our writer
        self.connect_stream_raw(stdout, stdin).await?;

        Ok(child)
    }

    /// Send a request and wait for response
    async fn request<T>(&self, request: ClientRequest) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.request_handler.request(request).await
    }

    /// Send a notification to the server
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        self.request_handler.send_notification(method, params).await
    }

    /// Invoke the connection callback if it has not run yet.
    async fn call_on_connect(&mut self) -> Result<()> {
        if self.on_connect_called {
            return Ok(());
        }

        let context = self.context.as_ref().ok_or_else(|| {
            Error::InternalError("Client context not available for on_connect".into())
        })?;
        self.connection.on_connect(context).await?;
        self.on_connect_called = true;
        Ok(())
    }

    /// Start the background task that handles incoming messages
    async fn start_message_handler(&mut self, stream: Box<dyn TransportStream>) -> Result<()> {
        let request_handler = self.request_handler.clone();

        // Split the transport stream into read and write halves
        let (tx, mut rx) = stream.split();

        // Wrap the sink in an Arc<Mutex> for sharing
        let tx = Arc::new(Mutex::new(tx));

        // Store the sender half for sending messages
        self.request_handler.set_transport(tx.clone());

        // Clone the connection for use in the handler
        let connection = self.connection.clone();

        // Create broadcast channel for client notifications
        let (client_notification_tx, mut client_notification_rx) = broadcast::channel(100);

        // Create the context for the connection
        let context = ClientCtx::new(client_notification_tx, Some(tx.clone()));
        self.context = Some(context.clone());

        // Clone sink for notification handler
        let notification_sink = tx.clone();

        // Spawn a task to handle incoming messages
        tokio::spawn(async move {
            debug!("Message handler started");

            loop {
                tokio::select! {
                    // Handle incoming messages from server
                    result = rx.next() => {
                        match result {
                            Some(Ok(message)) => {
                                debug!("Received message: {:?}", message);

                                match message {
                                    JSONRPCMessage::Response(response) => {
                                        request_handler.handle_response(response).await;
                                    }
                                    JSONRPCMessage::Notification(notification) => {
                                        // Convert the JSON-RPC notification into a typed
                                        // ServerNotification and pass it to the connection handler.
                                        if let Err(err) = handle_server_notification(&connection, &context, notification).await {
                                            error!("Failed to handle server notification: {}", err);
                                        }
                                    }
                                    JSONRPCMessage::Error(error) => {
                                        request_handler.handle_error(error).await;
                                    }
                                    JSONRPCMessage::Request(request) => {
                                        tracing::info!("Client received request from server: {:?}", request.id);
                                        let response = handle_server_request(&connection, &context, request).await;
                                        tracing::info!("Client sending response to server: {:?}", match &response {
                                            JSONRPCMessage::Response(r) => format!("{:?}", r.id),
                                            JSONRPCMessage::Error(e) => format!("{:?}", e.id),
                                            _ => "Unknown".to_string()
                                        });
                                        let mut sink = tx.lock().await;
                                        if let Err(e) = sink.send(response).await {
                                            error!("Failed to send response to server: {}", e);
                                            break;
                                        }
                                    }
                                    JSONRPCMessage::BatchRequest(_batch) => {
                                        // Clients typically don't receive batch requests from servers
                                        warn!("Received unexpected batch request from server");
                                    }
                                    JSONRPCMessage::BatchResponse(_batch) => {
                                        warn!(
                                            "Received batch response - batch requests not supported"
                                        );
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                error!("Error receiving message: {}", e);
                                break;
                            }
                            None => {
                                info!("Server disconnected");
                                break;
                            }
                        }
                    }

                    // Forward client notifications to server
                    result = client_notification_rx.recv() => {
                        match result {
                            Ok(notification) => {
                                let jsonrpc_notification = create_jsonrpc_notification(&notification);
                                let mut sink = notification_sink.lock().await;
                                if let Err(e) = sink.send(JSONRPCMessage::Notification(jsonrpc_notification)).await {
                                    error!("Error sending notification to server: {}", e);
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!("Client notification channel closed: {}", e);
                                // This is expected when the client shuts down
                            }
                        }
                    }
                }
            }

            // Clean up connection
            if let Err(e) = connection.on_shutdown(&context).await {
                error!("Error during client shutdown: {}", e);
            }

            info!("Message handler stopped");
        });

        Ok(())
    }
}

/// Implementation of ServerAPI trait for Client
#[async_trait]
impl<C> ServerAPI for Client<C>
where
    C: ClientHandler + Send + Sync + 'static,
{
    /// Initialize the connection with protocol version and capabilities
    async fn initialize(
        &mut self,
        protocol_version: String,
        capabilities: ClientCapabilities,
        client_info: Implementation,
    ) -> Result<InitializeResult> {
        let request = ClientRequest::Initialize {
            protocol_version,
            capabilities,
            client_info,
        };

        let result: InitializeResult = self.request(request).await?;

        // Send the initialized notification to complete the handshake
        self.send_notification("notifications/initialized", None)
            .await?;

        self.call_on_connect().await?;

        Ok(result)
    }

    /// Respond to ping requests
    async fn ping(&mut self) -> Result<()> {
        let _: EmptyResult = self.request(ClientRequest::Ping).await?;
        Ok(())
    }

    /// List available tools with optional pagination
    async fn list_tools(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListToolsResult> {
        let cursor: Option<Cursor> = cursor.into();
        self.request(ClientRequest::ListTools { cursor }).await
    }

    /// Call a tool with the given name and arguments
    async fn call_tool(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<CallToolResult> {
        let request = ClientRequest::CallTool {
            name: name.into(),
            arguments,
        };
        self.request(request).await
    }

    /// List available resources with optional pagination
    async fn list_resources(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListResourcesResult> {
        let cursor: Option<Cursor> = cursor.into();
        self.request(ClientRequest::ListResources { cursor }).await
    }

    /// List resource templates with optional pagination
    async fn list_resource_templates(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListResourceTemplatesResult> {
        let cursor: Option<Cursor> = cursor.into();
        self.request(ClientRequest::ListResourceTemplates { cursor })
            .await
    }

    /// Read a resource by URI
    async fn resources_read(
        &mut self,
        uri: impl Into<String> + Send,
    ) -> Result<ReadResourceResult> {
        self.request(ClientRequest::ReadResource { uri: uri.into() })
            .await
    }

    /// Subscribe to resource updates
    async fn resources_subscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: EmptyResult = self
            .request(ClientRequest::Subscribe { uri: uri.into() })
            .await?;
        Ok(())
    }

    /// Unsubscribe from resource updates
    async fn resources_unsubscribe(&mut self, uri: impl Into<String> + Send) -> Result<()> {
        let _: EmptyResult = self
            .request(ClientRequest::Unsubscribe { uri: uri.into() })
            .await?;
        Ok(())
    }

    /// List available prompts with optional pagination
    async fn list_prompts(
        &mut self,
        cursor: impl Into<Option<Cursor>> + Send,
    ) -> Result<ListPromptsResult> {
        let cursor: Option<Cursor> = cursor.into();
        self.request(ClientRequest::ListPrompts { cursor }).await
    }

    /// Get a prompt by name with optional arguments
    async fn get_prompt(
        &mut self,
        name: impl Into<String> + Send,
        arguments: Option<crate::Arguments>,
    ) -> Result<GetPromptResult> {
        self.request(ClientRequest::GetPrompt {
            name: name.into(),
            arguments,
        })
        .await
    }

    /// Handle completion requests
    async fn complete(
        &mut self,
        reference: Reference,
        argument: ArgumentInfo,
    ) -> Result<CompleteResult> {
        self.request(ClientRequest::Complete {
            reference,
            argument,
            context: None,
        })
        .await
    }

    /// Set the logging level
    async fn set_level(&mut self, level: LoggingLevel) -> Result<()> {
        let _: EmptyResult = self.request(ClientRequest::SetLevel { level }).await?;
        Ok(())
    }
}

/// Handle a request from the server using the ClientHandler trait
async fn handle_server_request<C: ClientHandler>(
    connection: &C,
    context: &ClientCtx,
    request: JSONRPCRequest,
) -> JSONRPCMessage {
    // Create a context with the request ID
    let ctx_with_request = context.with_request_id(request.id.clone());
    let result = handle_server_request_inner(connection, &ctx_with_request, request.clone()).await;
    result_to_jsonrpc_response(request.id, result)
}

/// Inner handler that returns Result<serde_json::Value>
async fn handle_server_request_inner<C: ClientHandler>(
    connection: &C,
    ctx: &ClientCtx,
    request: JSONRPCRequest,
) -> Result<serde_json::Value> {
    let mut request_obj = serde_json::Map::new();
    request_obj.insert(
        "method".to_string(),
        serde_json::Value::String(request.request.method.clone()),
    );
    if let Some(params) = request.request.params {
        for (key, value) in params.other {
            request_obj.insert(key, value);
        }
    }

    let server_request =
        match serde_json::from_value::<ServerRequest>(serde_json::Value::Object(request_obj)) {
            Ok(req) => req,
            Err(err) => {
                // Check if it's an unknown method or invalid parameters
                let err_str = err.to_string();
                if err_str.contains("unknown variant") {
                    return Err(Error::MethodNotFound(request.request.method.clone()));
                } else {
                    // It's a known method with invalid parameters
                    return Err(Error::InvalidParams(format!(
                        "Invalid parameters for {}: {}",
                        request.request.method, err
                    )));
                }
            }
        };

    match server_request {
        ServerRequest::Ping => {
            info!("Server sent ping request, sending pong");
            connection.pong(ctx).await.map(|_| serde_json::json!({}))
        }
        ServerRequest::CreateMessage(params) => connection
            .create_message(ctx, &request.request.method, *params)
            .await
            .and_then(|result| serde_json::to_value(result).map_err(Into::into)),
        ServerRequest::ListRoots => connection
            .list_roots(ctx)
            .await
            .and_then(|result| serde_json::to_value(result).map_err(Into::into)),
        ServerRequest::Elicit(params) => connection
            .elicit(ctx, params)
            .await
            .and_then(|result| serde_json::to_value(result).map_err(Into::into)),
    }
}

/// Convert a JSON-RPC notification into a typed ServerNotification and pass it
/// to the connection implementation for further handling.
async fn handle_server_notification<C: ClientHandler>(
    connection: &C,
    context: &ClientCtx,
    notification: JSONRPCNotification,
) -> Result<()> {
    // Build a serde_json::Value representing the notification in the shape
    // expected by the ServerNotification enum (which is `{"method": "...", ...}`).
    use serde_json::Value;

    let mut obj = serde_json::Map::new();
    obj.insert(
        "method".to_string(),
        Value::String(notification.notification.method.clone()),
    );

    if let Some(params) = notification.notification.params {
        for (k, v) in params.other {
            obj.insert(k, v);
        }
    }

    let value = Value::Object(obj);

    match serde_json::from_value::<ServerNotification>(value) {
        Ok(typed) => connection.notification(context, typed).await,
        Err(e) => Err(Error::InvalidParams(format!(
            "Failed to parse server notification: {e}",
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex as StdMutex},
        time::Instant,
    };

    use tokio::{
        io::duplex,
        process::Command,
        sync::{Mutex, oneshot},
        time::{Duration, sleep, timeout},
    };

    use super::*;

    #[test]
    fn test_pagination_api() {
        // This test just verifies the API is ergonomic - it doesn't run async code
        let mut client = Client::new("test-client", "1.0.0");

        // These should all compile cleanly
        drop(async {
            // Simple calls without cursors - passing None
            client.list_tools(None).await.unwrap();
            client.list_resources(None).await.unwrap();
            client.list_prompts(None).await.unwrap();
            client.list_resource_templates(None).await.unwrap();

            // Calls with cursor as Cursor type
            let cursor = Cursor::from("cursor");
            client.list_tools(cursor.clone()).await.unwrap();
            client.list_resources(cursor.clone()).await.unwrap();
            client.list_prompts(cursor.clone()).await.unwrap();
            client.list_resource_templates(cursor).await.unwrap();

            // Calls with explicit Some(Cursor)
            client
                .list_tools(Some(Cursor::from("some_cursor")))
                .await
                .unwrap();
            client
                .list_resources(Some(Cursor::from("some_cursor")))
                .await
                .unwrap();
            client
                .list_prompts(Some(Cursor::from("some_cursor")))
                .await
                .unwrap();
            client
                .list_resource_templates(Some(Cursor::from("some_cursor")))
                .await
                .unwrap();
        });
    }

    #[test]
    fn test_call_tool_api() {
        // This test just verifies the API is ergonomic - it doesn't run async code
        let mut client = Client::new("test-client", "1.0.0");

        // These should all compile cleanly
        drop(async {
            // Call without arguments - passing ()
            client.call_tool("my_tool", None).await.unwrap();

            // Call with String for tool name
            let tool_name = "another_tool".to_string();
            client.call_tool(tool_name, None).await.unwrap();

            // Call with &String
            let tool_name = "third_tool".to_string();
            client.call_tool(&tool_name, None).await.unwrap();

            // Call with HashMap arguments directly
            let mut args = HashMap::new();
            args.insert("param".to_string(), serde_json::json!("value"));
            client
                .call_tool("tool_with_args", Some(args.into()))
                .await
                .unwrap();

            // Call with Some(HashMap)
            let mut args = HashMap::new();
            args.insert("key".to_string(), serde_json::json!("value"));
            client
                .call_tool("tool_with_map", Some(args.into()))
                .await
                .unwrap();
        });
    }

    use crate::{
        connection::{ClientHandler as ClientHandlerTrait, ServerHandler as ServerHandlerTrait},
        context::{ClientCtx as ClientCtxType, ServerCtx},
        schema::{ClientNotification, ServerNotification},
        server::{Server, ServerHandle},
        transport::{GenericDuplex, StreamTransport, TestTransport},
    };

    async fn setup_client_server() -> (Client, ServerHandle) {
        // Create a minimal test connection
        #[derive(Debug, Default)]
        struct TestConnection;

        #[async_trait::async_trait]
        impl ServerHandlerTrait for TestConnection {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server").with_version("1.0.0"))
            }
        }

        let (client_transport, server_transport) = TestTransport::create_pair();

        let server = Server::default().with_handler(TestConnection::default);
        let server_handle = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start server");

        let mut client = Client::new("test-client", "1.0.0");
        client
            .connect(client_transport)
            .await
            .expect("Failed to connect");

        client.init().await.expect("Failed to initialize");

        (client, server_handle)
    }

    // Test that a ClientHandler implementation receives notifications sent
    // by the server.
    #[tokio::test]
    async fn test_client_receives_server_notification() {
        // Custom client connection that records notifications
        struct NotifClientHandler {
            tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
        }

        impl Clone for NotifClientHandler {
            fn clone(&self) -> Self {
                Self {
                    tx: self.tx.clone(),
                }
            }
        }

        #[async_trait::async_trait]
        impl ClientHandlerTrait for NotifClientHandler {
            async fn notification(
                &self,
                _context: &ClientCtxType,
                notification: ServerNotification,
            ) -> Result<()> {
                if matches!(notification, ServerNotification::ToolListChanged) {
                    let mut tx_guard = self.tx.lock().await;
                    if let Some(tx) = tx_guard.take() {
                        tx.send(()).ok();
                    }
                }
                Ok(())
            }
        }

        // Minimal server connection
        #[derive(Debug, Default)]
        struct DummyServerHandler;

        #[async_trait::async_trait]
        impl ServerHandlerTrait for DummyServerHandler {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server").with_version("1.0.0"))
            }
        }

        // Channel to signal when notification is received
        let (tx_notif, rx_notif) = oneshot::channel::<()>();

        // Create transport pair
        let (client_transport, server_transport) = TestTransport::create_pair();

        // Start server with tools/listChanged capability so notification is forwarded
        let server = Server::default()
            .with_handler(DummyServerHandler::default)
            .with_capabilities(ServerCapabilities::default().with_tools(Some(true)));
        let server_handle = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start server");

        // Create client with notif handler
        let mut client = Client::new("test-client", "1.0.0").with_handler(NotifClientHandler {
            tx: Arc::new(Mutex::new(Some(tx_notif))),
        });

        // Connect and initialize
        client
            .connect(client_transport)
            .await
            .expect("Failed to connect");

        client.init().await.expect("Failed to initialize");

        // Send server notification
        server_handle.send_server_notification(&ServerNotification::ToolListChanged);

        // Wait for notification to be received
        timeout(Duration::from_secs(1), rx_notif)
            .await
            .expect("Notification not received")
            .expect("Receiver dropped");
    }

    /// Ensure notifications are not forwarded when the server lacks the corresponding capability.
    #[tokio::test]
    async fn test_server_notification_filtered_without_capability() {
        struct NotifClientHandler {
            tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
        }

        impl Clone for NotifClientHandler {
            fn clone(&self) -> Self {
                Self {
                    tx: self.tx.clone(),
                }
            }
        }

        #[async_trait::async_trait]
        impl ClientHandlerTrait for NotifClientHandler {
            async fn notification(
                &self,
                _context: &ClientCtxType,
                _notification: ServerNotification,
            ) -> Result<()> {
                let mut tx_guard = self.tx.lock().await;
                if let Some(tx) = tx_guard.take() {
                    tx.send(()).ok();
                }
                Ok(())
            }
        }

        #[derive(Debug, Default)]
        struct DummyServerHandler;

        #[async_trait::async_trait]
        impl ServerHandlerTrait for DummyServerHandler {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server"))
            }
        }

        let (tx_notif, rx_notif) = oneshot::channel::<()>();

        let (client_transport, server_transport) = TestTransport::create_pair();

        // Start server without tools capability
        let server = Server::default().with_handler(DummyServerHandler::default);
        let server_handle = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start server");

        let mut client = Client::new("test-client", "1.0.0").with_handler(NotifClientHandler {
            tx: Arc::new(Mutex::new(Some(tx_notif))),
        });

        client
            .connect(client_transport)
            .await
            .expect("Failed to connect");

        client.init().await.expect("Failed to initialize");

        server_handle.send_server_notification(&ServerNotification::ToolListChanged);

        let res = timeout(Duration::from_millis(200), rx_notif).await;
        assert!(res.is_err(), "Notification should be filtered");
    }

    // Test that a ServerHandler implementation receives notifications sent
    // by the client.
    #[tokio::test]
    async fn test_server_receives_client_notification() {
        // Server connection that records notification
        #[derive(Debug, Default)]
        struct NotifyServerHandler {
            tx: Arc<StdMutex<Option<oneshot::Sender<()>>>>,
        }

        #[async_trait::async_trait]
        impl ServerHandlerTrait for NotifyServerHandler {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server").with_version("1.0.0"))
            }

            async fn notification(
                &self,
                _context: &ServerCtx,
                notification: ClientNotification,
            ) -> Result<()> {
                if matches!(notification, ClientNotification::Initialized) {
                    let maybe_tx = self.tx.lock().unwrap().take();
                    if let Some(tx) = maybe_tx {
                        tx.send(()).ok();
                    }
                }
                Ok(())
            }
        }

        // Client connection that sends a notification on connect
        #[derive(Clone)]
        struct NotifyClientHandler;

        #[async_trait::async_trait]
        impl ClientHandlerTrait for NotifyClientHandler {
            async fn on_connect(&self, context: &ClientCtxType) -> Result<()> {
                context.send_notification(ClientNotification::Initialized)?;
                Ok(())
            }
        }

        // Channel to notify when server receives notification
        let (tx_notif, rx_notif) = oneshot::channel();

        // Create transport pair
        let (client_transport, server_transport) = TestTransport::create_pair();

        let shared_tx: Arc<StdMutex<Option<oneshot::Sender<()>>>> =
            Arc::new(StdMutex::new(Some(tx_notif)));

        // Start server with notif connection
        let server = {
            let tx_clone = shared_tx.clone();
            Server::default().with_handler(move || NotifyServerHandler {
                tx: tx_clone.clone(),
            })
        };
        let server_handle = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start server");

        // Create client with notifier connection
        let mut client = Client::new("test-client", "1.0.0").with_handler(NotifyClientHandler);

        // Connect and initialize
        client
            .connect(client_transport)
            .await
            .expect("Failed to connect");

        client.init().await.expect("Failed to initialize");

        // Wait for server to receive notification
        timeout(Duration::from_secs(1), rx_notif)
            .await
            .expect("Server did not receive notification")
            .expect("Receiver dropped");
        drop(server_handle);
    }

    #[test]
    fn test_client_creation() {
        let client = Client::new("test-client", "1.0.0");
        assert_eq!(client.name, "test-client");
        assert_eq!(client.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_client_ping_server() {
        let (mut client, _server) = setup_client_server().await;
        client.ping().await.expect("Ping failed");
    }

    #[tokio::test]
    async fn test_multiple_client_pings() {
        let (mut client, _server) = setup_client_server().await;
        for i in 0..20 {
            client
                .ping()
                .await
                .unwrap_or_else(|_| panic!("Ping {i} failed"));
        }
    }

    #[tokio::test]
    async fn test_ping_performance() {
        let (mut client, _server) = setup_client_server().await;

        let start = Instant::now();
        let num_pings = 50;
        for _ in 0..num_pings {
            client.ping().await.expect("Ping failed");
        }

        let duration = start.elapsed();
        let pings_per_second = num_pings as f64 / duration.as_secs_f64();
        println!(
            "Client->Server: {num_pings} pings in {duration:?} ({pings_per_second:.1} pings/sec)"
        );
        assert!(
            pings_per_second > 50.0,
            "Too slow: {pings_per_second:.1} pings/sec"
        );
    }

    #[tokio::test]
    async fn test_connect_stream() {
        // Create a minimal test connection
        #[derive(Debug, Default)]
        struct TestStreamHandler;

        #[async_trait::async_trait]
        impl ServerHandlerTrait for TestStreamHandler {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server").with_version("1.0.0"))
            }
        }

        // Create a pair of duplex streams for testing
        let (client_reader, server_writer) = duplex(8192);
        let (server_reader, client_writer) = duplex(8192);

        // Create and configure server
        let server = Server::default()
            .with_handler(TestStreamHandler::default)
            .with_capabilities(ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(true),
                }),
                ..Default::default()
            });

        // Create server transport from the streams
        let server_duplex = GenericDuplex::new(server_reader, server_writer);
        let server_transport = Box::new(StreamTransport::new(server_duplex));

        let _server_handle = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start server");

        // Create and connect client using connect_stream
        let mut client = Client::new("test-client", "1.0.0");
        let result = client
            .connect_stream(client_reader, client_writer)
            .await
            .expect("Failed to connect client");

        assert_eq!(result.server_info.name, "test-server");

        // Test that we can ping
        client.ping().await.expect("Ping failed");
    }

    #[tokio::test]
    async fn test_connect_process() {
        // This test would require an actual MCP server binary to spawn
        // For now, we'll just test that the API compiles and handles errors correctly

        let mut client = Client::new("test-client", "1.0.0");

        // Try to spawn a non-existent process
        let cmd = Command::new("non-existent-mcp-server");
        let result = client.connect_process(cmd).await;

        // Should fail with a transport error
        assert!(result.is_err());
        if let Err(Error::Transport(msg)) = result {
            assert!(msg.contains("Failed to spawn process"));
        } else {
            panic!("Expected Transport error");
        }
    }

    #[tokio::test]
    async fn test_reconnect_works() {
        // Create a minimal test connection
        #[derive(Debug, Default)]
        struct TestConnection;

        #[async_trait::async_trait]
        impl ServerHandlerTrait for TestConnection {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: ClientCapabilities,
                _client_info: Implementation,
            ) -> Result<InitializeResult> {
                Ok(InitializeResult::new("test-server").with_version("1.0.0"))
            }
        }

        // Test that we can connect multiple times (e.g., after disconnect)
        let (mut client, server1) = setup_client_server().await;

        // First connection is already established by setup_client_server
        client.ping().await.expect("First ping failed");

        // Drop the first server to simulate disconnect
        drop(server1);

        // Wait a bit for the connection to close
        sleep(Duration::from_millis(100)).await;

        // Now create a new server and reconnect
        let (client_transport, server_transport) = TestTransport::create_pair();

        // Create a new test server
        let server = Server::default().with_handler(TestConnection::default);
        let _server2 = ServerHandle::new(server, server_transport)
            .await
            .expect("Failed to start second server");

        // Reconnect the client
        client
            .connect(client_transport)
            .await
            .expect("Reconnect failed");

        // Initialize and test the new connection
        client.init().await.expect("Re-initialize failed");
        client.ping().await.expect("Ping after reconnect failed");
    }
}
