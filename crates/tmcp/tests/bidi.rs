//! Tests for bidirectional communication between MCP client and server
//!
//! This test suite validates that:
//! 1. Clients can make requests to servers (normal flow)
//! 2. Servers can make requests to clients during request handling (reverse flow)
//! 3. Both directions support full request/response semantics
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tmcp::{
        Arguments, ClientCtx, ClientHandler, Result, ServerCtx, ServerHandler,
        schema::*,
        testutils::{connected_client_and_server_with_conn, shutdown_client_and_server},
    };
    use tracing_subscriber::fmt;

    /// Tracks method calls for test verification
    type CallTracker = Arc<Mutex<Vec<String>>>;

    /// Test client that can respond to server-initiated requests
    #[derive(Clone)]
    struct TestClient {
        calls: CallTracker,
    }

    impl TestClient {
        fn new() -> (Self, CallTracker) {
            let calls = CallTracker::default();
            (
                Self {
                    calls: calls.clone(),
                },
                calls,
            )
        }

        fn track_call(&self, method: &str) {
            self.calls.lock().unwrap().push(method.to_string());
        }
    }

    #[async_trait]
    impl ClientHandler for TestClient {
        async fn pong(&self, _context: &ClientCtx) -> Result<()> {
            self.track_call("client_pong");
            Ok(())
        }

        async fn list_roots(&self, _context: &ClientCtx) -> Result<ListRootsResult> {
            self.track_call("list_roots");
            Ok(ListRootsResult {
                roots: vec![Root {
                    uri: "file:///test".to_string(),
                    name: Some("Test Root".to_string()),
                    _meta: None,
                }],
                _meta: None,
            })
        }

        async fn create_message(
            &self,
            _context: &ClientCtx,
            _method: &str,
            params: CreateMessageParams,
        ) -> Result<CreateMessageResult> {
            self.track_call("create_message");

            let request_text = params
                .messages
                .first()
                .and_then(|msg| match &msg.content {
                    OneOrMany::One(block) => match block {
                        SamplingMessageContentBlock::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    },
                    OneOrMany::Many(blocks) => blocks.iter().find_map(|block| match block {
                        SamplingMessageContentBlock::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    }),
                })
                .unwrap_or("no message");

            Ok(CreateMessageResult {
                message: SamplingMessage {
                    role: Role::Assistant,
                    content: OneOrMany::One(SamplingMessageContentBlock::Text(TextContent {
                        text: format!("Client received: {request_text}"),
                        annotations: None,
                        _meta: None,
                    })),
                    _meta: None,
                },
                model: "test-model".to_string(),
                stop_reason: None,
            })
        }
    }

    /// Test server that initiates requests to the client mid-request
    #[derive(Clone)]
    struct TestServer {
        calls: CallTracker,
    }

    impl TestServer {
        fn new() -> (Self, CallTracker) {
            let calls = CallTracker::default();
            (
                Self {
                    calls: calls.clone(),
                },
                calls,
            )
        }

        fn track_call(&self, method: &str) {
            self.calls.lock().unwrap().push(method.to_string());
        }
    }

    #[async_trait]
    impl ServerHandler for TestServer {
        async fn pong(&self, _context: &ServerCtx) -> Result<()> {
            self.track_call("server_pong");
            Ok(())
        }

        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("test-server").with_version("1.0.0"))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResult> {
            self.track_call(&format!("tool_{name}"));

            match name.as_str() {
                "ping_client" => {
                    let mut ctx = context.clone();
                    ctx.ping().await?;
                    Ok(CallToolResult::new().with_text_content("Client pinged"))
                }

                "query_client_roots" => {
                    let mut ctx = context.clone();
                    let roots = ctx.list_roots().await?;
                    Ok(CallToolResult::new()
                        .with_text_content(format!("Found {} client roots", roots.roots.len())))
                }

                "ask_client_to_generate" => {
                    let params = CreateMessageParams {
                        messages: vec![SamplingMessage {
                            role: Role::User,
                            content: OneOrMany::One(SamplingMessageContentBlock::Text(
                                TextContent {
                                    text: "Server request".to_string(),
                                    annotations: None,
                                    _meta: None,
                                },
                            )),
                            _meta: None,
                        }],
                        system_prompt: None,
                        include_context: None,
                        temperature: None,
                        max_tokens: 100,
                        metadata: None,
                        stop_sequences: None,
                        model_preferences: None,
                        tools: None,
                        tool_choice: None,
                        task: None,
                        _meta: None,
                    };

                    let mut ctx = context.clone();
                    let result = ctx.create_message(params).await?;
                    let text = match result.message.content {
                        OneOrMany::One(SamplingMessageContentBlock::Text(text)) => text.text,
                        OneOrMany::Many(blocks) => blocks
                            .into_iter()
                            .find_map(|block| match block {
                                SamplingMessageContentBlock::Text(text) => Some(text.text),
                                _ => None,
                            })
                            .unwrap_or_else(|| "Non-text response".to_string()),
                        _ => "Non-text response".to_string(),
                    };
                    Ok(CallToolResult::new().with_text_content(text))
                }

                _ => Err(tmcp::Error::ToolExecutionFailed {
                    tool: name,
                    message: "Unknown tool".to_string(),
                }),
            }
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            Ok(ListToolsResult::new()
                .with_tool(
                    Tool::new("ping_client", ToolSchema::default())
                        .with_description("Ping the client during request handling"),
                )
                .with_tool(
                    Tool::new("query_client_roots", ToolSchema::default())
                        .with_description("Query client's file roots during request handling"),
                )
                .with_tool(
                    Tool::new("ask_client_to_generate", ToolSchema::default()).with_description(
                        "Ask client to generate a message during request handling",
                    ),
                ))
        }
    }

    #[tokio::test]
    async fn test_server_calls_client_during_request() {
        fmt::try_init().ok();

        // Create test client and server with call tracking
        let (test_client, client_calls) = TestClient::new();
        let (test_server, server_calls) = TestServer::new();

        let (mut client, server_handle) = connected_client_and_server_with_conn(
            move || Box::new(test_server.clone()),
            test_client,
        )
        .await
        .expect("Failed to create client/server pair");

        // Initialize connection
        client
            .initialize(
                LATEST_PROTOCOL_VERSION.to_string(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .expect("Initialize failed");

        // Test 1: Server pings client during tool execution
        client_calls.lock().unwrap().clear();
        client
            .call_tool("ping_client", None, None)
            .await
            .expect("ping_client tool failed");

        assert_eq!(
            client_calls.lock().unwrap().as_slice(),
            &["client_pong"],
            "Server should have pinged client"
        );

        // Test 2: Server queries client roots during tool execution
        client_calls.lock().unwrap().clear();
        let result = client
            .call_tool("query_client_roots", None, None)
            .await
            .expect("query_client_roots tool failed");

        assert_eq!(
            client_calls.lock().unwrap().as_slice(),
            &["list_roots"],
            "Server should have queried client roots"
        );

        if let Some(ContentBlock::Text(text)) = result.content.first() {
            assert!(text.text.contains("1 client roots"));
        }

        // Test 3: Server asks client to generate message during tool execution
        client_calls.lock().unwrap().clear();
        let result = client
            .call_tool("ask_client_to_generate", None, None)
            .await
            .expect("ask_client_to_generate tool failed");

        assert_eq!(
            client_calls.lock().unwrap().as_slice(),
            &["create_message"],
            "Server should have asked client to create message"
        );

        if let Some(ContentBlock::Text(text)) = result.content.first() {
            assert_eq!(text.text, "Client received: Server request");
        }

        // Verify server tracked all tool calls
        {
            let server_call_log = server_calls.lock().unwrap();
            assert_eq!(
                server_call_log.as_slice(),
                &[
                    "tool_ping_client",
                    "tool_query_client_roots",
                    "tool_ask_client_to_generate"
                ],
                "Server should have tracked all tool calls"
            );
        }

        shutdown_client_and_server(client, server_handle).await;
    }

    #[tokio::test]
    async fn test_client_server_ping_pong() {
        fmt::try_init().ok();

        let (test_client, client_calls) = TestClient::new();
        let (test_server, server_calls) = TestServer::new();

        let (mut client, server_handle) = connected_client_and_server_with_conn(
            move || Box::new(test_server.clone()),
            test_client,
        )
        .await
        .expect("Failed to create client/server pair");

        client
            .initialize(
                LATEST_PROTOCOL_VERSION.to_string(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .expect("Initialize failed");

        // Client pings server (normal direction)
        client.ping().await.expect("Client->Server ping failed");
        assert!(
            server_calls
                .lock()
                .unwrap()
                .contains(&"server_pong".to_string()),
            "Server should respond to client ping"
        );

        // Server pings client (reverse direction via tool call)
        client_calls.lock().unwrap().clear();
        client
            .call_tool("ping_client", None, None)
            .await
            .expect("Server->Client ping failed");
        assert!(
            client_calls
                .lock()
                .unwrap()
                .contains(&"client_pong".to_string()),
            "Client should respond to server ping"
        );

        shutdown_client_and_server(client, server_handle).await;
    }
}
