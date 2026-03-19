//! Regression tests for in-flight requests during client disconnect.

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tmcp::{
        Arguments, Result, Server, ServerCtx, ServerHandler,
        schema::{
            CallToolResult, ClientCapabilities, Cursor, Implementation, InitializeResult,
            LATEST_PROTOCOL_VERSION, ListToolsResult, ServerCapabilities, TaskMetadata, Tool,
            ToolSchema,
        },
        testutils::make_duplex_pair,
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        time::{Duration, sleep, timeout},
    };

    struct SlowToolServer;

    #[async_trait]
    impl ServerHandler for SlowToolServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("slow-tool-server")
                .with_version("0.1.0")
                .with_capabilities(ServerCapabilities::default().with_tools(None)))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            Ok(ListToolsResult::default().with_tool(Tool::new("slow_echo", ToolSchema::default())))
        }

        async fn call_tool(
            &self,
            _context: &ServerCtx,
            name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResult> {
            assert_eq!(name, "slow_echo");
            sleep(Duration::from_millis(50)).await;
            Ok(CallToolResult::new().with_text_content("ok"))
        }
    }

    #[tokio::test]
    async fn request_response_survives_client_write_disconnect() {
        let server = Server::new(|| SlowToolServer);
        let (server_reader, server_writer, client_reader, mut client_writer) = make_duplex_pair();
        let server_handle = tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .expect("server starts");
        let server_task = server_handle.handle;

        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "0.1.0" }
            }
        });
        let initialized = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        });
        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "slow_echo",
                "arguments": {}
            }
        });

        client_writer
            .write_all(format!("{initialize}\n{initialized}\n{call}\n").as_bytes())
            .await
            .expect("requests written");
        client_writer.flush().await.expect("requests flushed");
        client_writer.shutdown().await.expect("writer shutdown");
        drop(client_writer);

        let mut reader = BufReader::new(client_reader);
        let mut lines = Vec::new();
        let deadline = Duration::from_secs(2);

        timeout(deadline, async {
            loop {
                let mut line = String::new();
                let n = reader
                    .read_line(&mut line)
                    .await
                    .expect("read response line");
                if n == 0 {
                    break;
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                lines.push(line.to_string());
                let value: Value = serde_json::from_str(line).expect("valid json-rpc");
                if value.get("id") == Some(&json!(2)) {
                    let text = value["result"]["content"][0]["text"]
                        .as_str()
                        .expect("tool text response");
                    assert!(text.contains("\"success\":true") || text == "ok");
                    break;
                }
            }
        })
        .await
        .expect("response received before timeout");

        assert!(
            lines.iter().any(|line| line.contains("\"id\":2")),
            "server must send the in-flight response after client EOF"
        );

        timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task finished")
            .expect("server task succeeded");
    }
}
