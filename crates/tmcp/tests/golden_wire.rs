//! Golden wire-format tests.
//!
//! These drive a deterministic server with raw JSON-RPC fixtures from
//! `tests/test_data/golden/` and assert the exact JSON the server sends back.
//! Round-tripping through the Rust types cannot catch wire-format bugs (a
//! wrong field name deserializes back perfectly), so protocol fidelity is
//! locked in here instead.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use serde_json::json;
    use tmcp::{
        Arguments, Error, Result, ServerCtx, ServerHandler,
        schema::{self, ProtocolVersion},
        testutils::run_wire_fixture,
    };

    /// A server whose responses are fully deterministic, so fixtures can
    /// assert exact JSON.
    struct GoldenServer;

    #[async_trait]
    impl ServerHandler for GoldenServer {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: schema::ClientCapabilities,
            _client_info: schema::Implementation,
        ) -> Result<schema::InitializeResult> {
            Ok(schema::InitializeResult::new("golden-server")
                .with_version("1.0.0")
                .with_instructions("Golden wire-format test server.")
                .with_capabilities(
                    schema::ServerCapabilities::default()
                        .with_logging()
                        .with_prompts(Some(false))
                        .with_resources(None, Some(false))
                        .with_tools(Some(true)),
                ))
        }

        async fn list_tools(
            &self,
            _ctx: &ServerCtx,
            _cursor: Option<schema::Cursor>,
        ) -> Result<schema::ListToolsResult> {
            let input_schema = schema::ToolSchema(json!({
                "type": "object",
                "properties": { "message": { "type": "string" } },
                "required": ["message"],
            }));
            let tool = schema::Tool::new("echo", input_schema)
                .with_description("Echo a message back.")
                .with_annotations(schema::ToolAnnotations {
                    title: Some("Echo".to_string()),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                });
            Ok(schema::ListToolsResult::new().with_tool(tool))
        }

        async fn call_tool(
            &self,
            _ctx: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
            _task: Option<schema::TaskMetadata>,
        ) -> Result<schema::CallToolResponse> {
            if name != "echo" {
                return Err(Error::ToolNotFound(name));
            }
            let message: String = arguments
                .and_then(|args| args.get("message"))
                .unwrap_or_default();
            Ok(schema::CallToolResponse::result(
                schema::CallToolResult::new().with_text_content(format!("echo: {message}")),
            ))
        }

        async fn list_resources(
            &self,
            _ctx: &ServerCtx,
            _cursor: Option<schema::Cursor>,
        ) -> Result<schema::ListResourcesResult> {
            let resource = schema::Resource::new("greeting", "golden://greeting")
                .with_description("A canned greeting.")
                .with_mime_type("text/plain");
            Ok(schema::ListResourcesResult::new().with_resource(resource))
        }

        async fn read_resource(
            &self,
            _ctx: &ServerCtx,
            uri: String,
        ) -> Result<schema::ReadResourceResult> {
            if uri != "golden://greeting" {
                return Err(Error::ResourceNotFound { uri });
            }
            Ok(schema::ReadResourceResult::new().with_text(uri, "Hello, golden world!"))
        }

        async fn list_prompts(
            &self,
            _ctx: &ServerCtx,
            _cursor: Option<schema::Cursor>,
        ) -> Result<schema::ListPromptsResult> {
            let prompt = schema::Prompt::new("greet")
                .with_description("Greet someone by name.")
                .with_argument(schema::PromptArgument::new("name").with_required(true));
            Ok(schema::ListPromptsResult::new().with_prompt(prompt))
        }

        async fn get_prompt(
            &self,
            _ctx: &ServerCtx,
            name: String,
            arguments: Option<HashMap<String, String>>,
        ) -> Result<schema::GetPromptResult> {
            if name != "greet" {
                return Err(Error::PromptNotFound(name));
            }
            let who = arguments
                .and_then(|args| args.get("name").cloned())
                .unwrap_or_default();
            Ok(schema::GetPromptResult::new()
                .with_description("Greet someone by name.")
                .with_message(schema::PromptMessage::new(
                    schema::Role::User,
                    schema::ContentBlock::text(format!("Greet {who}.")),
                )))
        }
    }

    #[tokio::test]
    async fn malformed_line_yields_parse_error_and_connection_survives() {
        use serde_json::json;
        use tmcp::testutils::WireConnection;

        let mut conn = WireConnection::start(|| GoldenServer)
            .await
            .expect("start wire server");

        conn.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "golden-client", "version": "1.0.0" }
            }
        }))
        .await;
        conn.recv().await;
        conn.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;

        // A garbage line earns a -32700 with a null id...
        conn.send_raw("this is not json").await;
        let response = conn.recv().await;
        assert_eq!(response["error"]["code"], -32700);
        assert_eq!(response["id"], serde_json::Value::Null);

        // ...and the connection keeps working afterwards.
        conn.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }))
            .await;
        let response = conn.recv().await;
        assert_eq!(response["id"], 2);
        assert!(response["result"].is_object());

        // Repeated initialization is rejected without killing the connection.
        conn.send(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "golden-client", "version": "1.0.0" }
            }
        }))
        .await;
        let response = conn.recv().await;
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["id"], 3);
    }

    #[tokio::test]
    async fn invalid_protocol_version_fails_before_negotiation() {
        use serde_json::json;
        use tmcp::testutils::WireConnection;

        let mut conn = WireConnection::start(|| GoldenServer)
            .await
            .expect("start wire server");
        conn.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-02-29",
                "capabilities": {},
                "clientInfo": { "name": "golden-client", "version": "1.0.0" }
            }
        }))
        .await;

        let response = conn.recv().await;
        assert_eq!(response["error"]["code"], -32602);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid MCP protocol version")
        );
    }

    #[tokio::test]
    async fn golden_lifecycle() {
        run_wire_fixture(
            || GoldenServer,
            include_str!("test_data/golden/lifecycle.json"),
        )
        .await;
    }

    #[tokio::test]
    async fn golden_tools() {
        run_wire_fixture(|| GoldenServer, include_str!("test_data/golden/tools.json")).await;
    }

    #[tokio::test]
    async fn golden_resources_prompts() {
        run_wire_fixture(
            || GoldenServer,
            include_str!("test_data/golden/resources_prompts.json"),
        )
        .await;
    }
}
