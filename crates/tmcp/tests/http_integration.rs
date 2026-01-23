//! HTTP transport integration tests.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use serde_json::json;
    use tmcp::{
        Arguments, Client, Result, Server, ServerCtx, ServerHandler, ToolError,
        schema::{self, *},
    };
    use tokio::time::{Duration, sleep};
    use tracing_subscriber::fmt;

    #[derive(Default)]
    struct EchoConnection;

    #[async_trait]
    impl ServerHandler for EchoConnection {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("http-echo-server")
                .with_version("0.1.0")
                .with_capabilities(ServerCapabilities::default().with_tools(None)))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            let schema = ToolSchema::default()
                .with_property("message", json!({"type": "string"}))
                .with_required("message");
            Ok(ListToolsResult::default()
                .with_tool(Tool::new("echo", schema).with_description("Echo message")))
        }

        async fn call_tool(
            &self,
            _context: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResult> {
            if name != "echo" {
                return Err(tmcp::Error::ToolNotFound(name));
            }
            let Some(args) = arguments else {
                return Ok(ToolError::invalid_input("Missing args").into());
            };
            let Some(message) = args.get_string("message") else {
                return Ok(ToolError::invalid_input("Missing message").into());
            };
            Ok(CallToolResult::new().with_text_content(message))
        }
    }

    #[tokio::test]
    async fn test_http_echo_tool_integration() {
        fmt::try_init().ok();

        // Use port 0 to let the OS assign an available port
        let server_handle = Server::new(EchoConnection::default)
            .serve_http("127.0.0.1:0")
            .await
            .unwrap();

        // Get the actual bound address
        let bound_addr = server_handle.bound_addr.as_ref().unwrap();
        // Small delay to ensure server is fully ready
        sleep(Duration::from_millis(100)).await;

        // Connect HTTP client
        let mut client = Client::new("http-test-client", "0.1.0");
        let init = client
            .connect_http(&format!("http://{bound_addr}"))
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "http-echo-server");

        // Call echo tool - HashMap implements Serialize so can be passed directly
        let mut args = HashMap::new();
        args.insert("message".to_string(), json!("hello"));
        let result = client.call_tool("echo", args).await.unwrap();
        if let Some(schema::ContentBlock::Text(text)) = result.content.first() {
            assert_eq!(text.text, "hello");
        } else {
            panic!("expected text response");
        }

        drop(client);
        // Properly stop the server
        server_handle.stop().await.unwrap();
    }
}
