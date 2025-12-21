//! Integration tests for rmcp and tenx-mcp interoperability
//!
//! This module contains actual integration tests that verify both
//! implementations can communicate with each other correctly.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, result::Result as StdResult, sync::Arc};

    use async_trait::async_trait;
    use rmcp::{ServiceExt, model as rmcp_model};
    use rmcp_model::{CallToolRequestParam, PaginatedRequestParam};
    use serde_json::json;
    use tenx_mcp::{
        Arguments, Client, Error, Result, Server, ServerAPI, ServerConn, ServerCtx, schema::*,
        testutils::make_duplex_pair,
    };
    use tokio::{
        io::duplex,
        time::{Duration, sleep, timeout},
    };
    use tracing_subscriber::fmt;

    struct EchoConnection;

    #[async_trait]
    impl ServerConn for EchoConnection {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("test-server")
                .with_version("0.1.0")
                .with_tools(true))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            tracing::info!("EchoConnection.tools_list called");
            let schema = ToolSchema {
                schema_type: "object".to_string(),
                properties: Some({
                    let mut props = HashMap::new();
                    props.insert(
                        "message".to_string(),
                        json!({
                            "type": "string",
                            "description": "The message to echo"
                        }),
                    );
                    props
                }),
                required: Some(vec!["message".to_string()]),
            };

            Ok(ListToolsResult::new()
                .with_tool(Tool::new("echo", schema).with_description("Echoes the input message")))
        }

        async fn call_tool(
            &self,
            _context: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
        ) -> Result<CallToolResult> {
            if name != "echo" {
                return Err(Error::ToolExecutionFailed {
                    tool: name,
                    message: "Tool not found".to_string(),
                });
            }

            let args = arguments
                .ok_or_else(|| Error::InvalidParams("echo: Missing arguments".to_string()))?;
            let message = args.get_string("message").ok_or_else(|| {
                Error::InvalidParams("echo: Missing message parameter".to_string())
            })?;

            Ok(CallToolResult {
                content: vec![Content::Text(TextContent {
                    text: message,
                    annotations: None,
                    _meta: None,
                })],
                is_error: Some(false),
                structured_content: None,
                _meta: None,
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_tenx_server_with_rmcp_client() {
        // Initialize a tracing subscriber so that we get helpful debug output if
        // this test fails or hangs. We deliberately call `try_init` so that it's
        // no-op when a subscriber has already been installed by another test.
        fmt::try_init().ok();
        // Create bidirectional streams for communication using the shared test
        // utility.
        let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();

        // Create and configure tenx-mcp server
        let server = Server::default()
            .with_connection(|| EchoConnection)
            .with_capabilities(ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(true),
                }),
                resources: None,
                prompts: None,
                logging: None,
                completions: None,
                experimental: None,
            });

        // Start tenx-mcp server in background using the new serve_stream method
        let server_handle =
            tenx_mcp::ServerHandle::from_stream(server, server_reader, server_writer)
                .await
                .expect("Failed to start server");

        // Give server time to start
        sleep(Duration::from_millis(100)).await;

        // Create rmcp client using the streams
        let client_transport = (client_reader, client_writer);

        // Connect rmcp client - initialization happens automatically
        let client = timeout(Duration::from_secs(5), ().serve(client_transport))
            .await
            .expect("Client connection timed out")
            .expect("Failed to connect client");

        // List tools
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "echo");

        // Call echo tool
        let mut args = serde_json::Map::new();
        args.insert("message".to_string(), json!("Hello from rmcp!"));

        let result = client
            .call_tool(rmcp_model::CallToolRequestParam {
                name: "echo".into(),
                arguments: Some(args),
            })
            .await
            .unwrap();

        // Verify result
        assert_eq!(result.content.len(), 1);
        match &result.content[0].raw {
            rmcp_model::RawContent::Text(text_content) => {
                assert_eq!(&text_content.text, "Hello from rmcp!");
            }
            _ => panic!("Expected text content"),
        }

        // Cleanup: we drop the client first so that the underlying transport is
        // closed and the server task can finish. To avoid hanging the test in the
        // unlikely case that it doesn't shut down promptly, we wrap the wait in a
        // short timeout.
        drop(client);

        // Give the server task a moment to observe the closed connection and shut
        // itself down. We ignore any timeout errors here because the important
        // part of the test (inter-operability) has already completed.
        timeout(Duration::from_millis(100), server_handle.stop())
            .await
            .ok();
    }

    #[tokio::test]
    async fn test_rmcp_server_with_tenx_client() {
        use rmcp::{
            handler::server::ServerHandler,
            service::{RequestContext, RoleServer},
        };

        // Create a simple rmcp server
        #[derive(Debug, Clone)]
        struct TestRmcpServer;

        impl ServerHandler for TestRmcpServer {
            async fn initialize(
                &self,
                _request: rmcp_model::InitializeRequestParam,
                _ctx: RequestContext<RoleServer>,
            ) -> StdResult<rmcp_model::InitializeResult, rmcp::ErrorData> {
                Ok(rmcp_model::InitializeResult {
                    protocol_version: rmcp_model::ProtocolVersion::default(),
                    capabilities: rmcp_model::ServerCapabilities::default(),
                    server_info: rmcp_model::Implementation {
                        name: "test-rmcp-server".to_string(),
                        title: None,
                        version: "0.1.0".to_string(),
                        icons: None,
                        website_url: None,
                    },
                    instructions: None,
                })
            }

            async fn list_tools(
                &self,
                _params: Option<PaginatedRequestParam>,
                _ctx: RequestContext<RoleServer>,
            ) -> StdResult<rmcp_model::ListToolsResult, rmcp::ErrorData> {
                Ok(rmcp_model::ListToolsResult {
                    meta: None,
                    next_cursor: None,
                    tools: vec![rmcp_model::Tool {
                        name: "reverse".into(),
                        title: None,
                        description: Some("Reverses a string".into()),
                        input_schema: {
                            let mut schema = serde_json::Map::new();
                            schema.insert("type".to_string(), json!("object"));

                            let mut properties = serde_json::Map::new();
                            properties.insert(
                                "text".to_string(),
                                json!({
                                    "type": "string",
                                    "description": "Text to reverse"
                                }),
                            );
                            schema.insert("properties".to_string(), json!(properties));
                            schema.insert("required".to_string(), json!(["text"]));
                            Arc::new(schema)
                        },
                        output_schema: None,
                        annotations: None,
                        icons: None,
                        meta: None,
                    }],
                })
            }

            async fn call_tool(
                &self,
                params: CallToolRequestParam,
                _ctx: RequestContext<RoleServer>,
            ) -> StdResult<rmcp_model::CallToolResult, rmcp::ErrorData> {
                if params.name == "reverse" {
                    let text = params
                        .arguments
                        .as_ref()
                        .and_then(|args| args.get("text"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            rmcp::ErrorData::invalid_params(
                                "reverse: Missing text parameter",
                                None,
                            )
                        })?;

                    let reversed = text.chars().rev().collect::<String>();

                    Ok(rmcp_model::CallToolResult {
                        content: vec![rmcp_model::Content::text(reversed)],
                        structured_content: None,
                        is_error: None,
                        meta: None,
                    })
                } else {
                    Err(rmcp::ErrorData::invalid_request("Unknown tool", None))
                }
            }
        }

        // Create bidirectional streams
        let (client_reader, server_writer) = duplex(8192);
        let (server_reader, client_writer) = duplex(8192);

        // Start rmcp server
        let server = TestRmcpServer;
        let server_handle = tokio::spawn(async move {
            let transport = (server_reader, server_writer);
            let _service = server.serve(transport).await.unwrap();
            // Keep the server running
            sleep(Duration::from_secs(10)).await;
        });

        // Give server time to start
        sleep(Duration::from_millis(100)).await;

        // Create tenx-mcp client
        let mut client = Client::new("test-client", "0.1.0");
        client
            .connect_stream(client_reader, client_writer)
            .await
            .unwrap();

        // Initialize
        let init_result = client.init().await.unwrap();

        // Check server info is valid
        assert!(!init_result.server_info.name.is_empty());

        // List tools from rmcp server
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "reverse");

        // Call reverse tool
        let mut args = HashMap::new();
        args.insert("text".to_string(), json!("hello"));
        let result = client
            .call_tool("reverse", Some(args.into()))
            .await
            .unwrap();

        // Verify reversed result
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            Content::Text(text) => {
                assert_eq!(text.text, "olleh");
            }
            _ => panic!("Expected text content"),
        }

        // Cleanup
        server_handle.abort();
    }
}
