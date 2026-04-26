//! Macro derive tests for the tmcp procedural macros.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use tmcp::{
        Error, Result, ServerCtx, ServerHandler, ToolResponse, ToolResult, mcp_server, schema::*,
        testutils::TestServerContext, tool,
    };

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct EchoParams {
        message: String,
    }

    #[derive(Debug, Serialize, Deserialize, JsonSchema)]
    struct AddParams {
        a: f64,
        b: f64,
    }

    #[derive(Debug, Serialize, ToolResponse, JsonSchema)]
    struct PingResponse {
        message: String,
    }

    #[derive(Debug, Default)]
    struct TestServer;

    #[mcp_server]
    /// Test server with echo and add tools
    impl TestServer {
        #[tool]
        /// Echo the message
        async fn echo(&self, _ctx: &tmcp::ServerCtx, params: EchoParams) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(params.message))
        }

        #[tool]
        /// Add two numbers
        async fn add(&self, _ctx: &tmcp::ServerCtx, params: AddParams) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(format!("{}", params.a + params.b)))
        }

        #[tool]
        /// Multiply two numbers
        async fn multiply(&self, _ctx: &tmcp::ServerCtx, a: f64, b: f64) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(format!("{}", a * b)))
        }

        #[tool]
        /// Ping the server
        async fn ping(&self, _ctx: &tmcp::ServerCtx) -> ToolResult<PingResponse> {
            Ok(PingResponse {
                message: "pong".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn test_initialize() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        let result = server
            .initialize(
                ctx.ctx(),
                "1.0.0".to_string(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        assert_eq!(result.server_info.name, "test_server");
        assert_eq!(
            result.instructions,
            Some("Test server with echo and add tools".to_string())
        );
    }

    #[tokio::test]
    async fn test_list_tools() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        let result = server.list_tools(ctx.ctx(), None).await.unwrap();

        assert_eq!(result.tools.len(), 4);
        assert!(
            result
                .tools
                .iter()
                .any(|t| t.name == "echo" && t.description == Some("Echo the message".to_string()))
        );
        assert!(
            result
                .tools
                .iter()
                .any(|t| t.name == "add" && t.description == Some("Add two numbers".to_string()))
        );
        assert!(
            result.tools.iter().any(|t| t.name == "multiply"
                && t.description == Some("Multiply two numbers".to_string()))
        );
        assert!(
            result
                .tools
                .iter()
                .any(|t| t.name == "ping" && t.description == Some("Ping the server".to_string()))
        );
    }

    #[tokio::test]
    async fn test_call_tools() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        // Test echo
        let mut args = HashMap::new();
        args.insert("message".to_string(), serde_json::json!("hello"));

        let result = server
            .call_tool(ctx.ctx(), "echo".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "hello"),
            _ => panic!("Expected text content"),
        }

        // Test add
        let mut args = HashMap::new();
        args.insert("a".to_string(), serde_json::json!(3.5));
        args.insert("b".to_string(), serde_json::json!(2.5));

        let result = server
            .call_tool(ctx.ctx(), "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "6"),
            _ => panic!("Expected text content"),
        }

        // Test multiply
        let mut args = HashMap::new();
        args.insert("a".to_string(), serde_json::json!(3.0));
        args.insert("b".to_string(), serde_json::json!(4.0));

        let result = server
            .call_tool(ctx.ctx(), "multiply".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "12"),
            _ => panic!("Expected text content"),
        }

        let result = server
            .call_tool(ctx.ctx(), "ping".to_string(), None, None)
            .await
            .unwrap();
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "message": "pong" }))
        );
    }

    #[tokio::test]
    async fn test_error_handling() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        // Unknown tool
        let err = server
            .call_tool(ctx.ctx(), "unknown".to_string(), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::ToolNotFound(_)));

        // Missing arguments
        let result = server
            .call_tool(ctx.ctx(), "echo".to_string(), None, None)
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));

        // Invalid arguments
        let mut args = HashMap::new();
        args.insert("a".to_string(), serde_json::json!("not a number"));
        args.insert("b".to_string(), serde_json::json!(2.0));

        let result = server
            .call_tool(ctx.ctx(), "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    // Test for custom initialize function
    #[derive(Debug, Default)]
    struct CustomInitServer;

    #[mcp_server(initialize_fn = custom_init)]
    /// Server with custom initialization
    impl CustomInitServer {
        async fn custom_init(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("custom_init_server")
                .with_version("2.0.0")
                .with_tools(true)
                .with_instructions("Custom initialized server"))
        }

        #[tool]
        /// A simple test tool
        async fn test_tool(&self, _ctx: &ServerCtx, params: EchoParams) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(format!("Custom: {}", params.message)))
        }
    }

    #[tokio::test]
    async fn test_custom_initialize() {
        let server = CustomInitServer;
        let ctx = TestServerContext::new();

        let result = server
            .initialize(
                ctx.ctx(),
                "1.0.0".to_string(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        // Verify custom initialization was used
        assert_eq!(result.server_info.name, "custom_init_server");
        assert_eq!(result.server_info.version, "2.0.0");
        assert_eq!(result.protocol_version, LATEST_PROTOCOL_VERSION);
        assert_eq!(
            result.instructions,
            Some("Custom initialized server".to_string())
        );

        // Verify custom capabilities
        let tools_cap = result.capabilities.tools.unwrap();
        assert_eq!(tools_cap.list_changed, Some(true));
    }

    #[tokio::test]
    async fn test_custom_init_with_tools() {
        let server = CustomInitServer;
        let ctx = TestServerContext::new();

        // Verify tools still work with custom init
        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "test_tool");

        // Test calling the tool
        let mut args = HashMap::new();
        args.insert("message".to_string(), serde_json::json!("test"));

        let result = server
            .call_tool(ctx.ctx(), "test_tool".to_string(), Some(args.into()), None)
            .await
            .unwrap();

        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "Custom: test"),
            _ => panic!("Expected text content"),
        }
    }

    #[derive(Debug)]
    struct DynamicResourceServer {
        docs: HashMap<String, String>,
    }

    impl Default for DynamicResourceServer {
        fn default() -> Self {
            Self {
                docs: HashMap::from([(
                    "tmcp://api/echo.d.luau".to_string(),
                    "declare function echo(message: string): string".to_string(),
                )]),
            }
        }
    }

    #[mcp_server(
        resources_fn = list_docs,
        read_resource_fn = read_doc,
        resource_templates_fn = list_doc_templates
    )]
    /// Server with dynamic resources only
    impl DynamicResourceServer {
        async fn list_docs(
            &self,
            _ctx: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListResourcesResult> {
            Ok(
                ListResourcesResult::new().with_resources(self.docs.keys().map(|uri| {
                    Resource::new("Luau API", uri).with_mime_type("application/luau-definitions")
                })),
            )
        }

        async fn read_doc(&self, _ctx: &ServerCtx, uri: String) -> Result<ReadResourceResult> {
            let Some(source) = self.docs.get(&uri) else {
                return Err(Error::ResourceNotFound { uri });
            };

            Ok(
                ReadResourceResult::new().with_content(ResourceContents::Text(
                    TextResourceContents::new(uri, source.clone())
                        .with_mime_type("application/luau-definitions"),
                )),
            )
        }

        async fn list_doc_templates(
            &self,
            _ctx: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListResourceTemplatesResult> {
            Ok(ListResourceTemplatesResult::new().with_resource_template(
                ResourceTemplate::new("Luau API", "tmcp://api/{tool}.d.luau")
                    .with_mime_type("application/luau-definitions"),
            ))
        }
    }

    #[tokio::test]
    async fn test_dynamic_resource_callbacks() {
        let server = DynamicResourceServer::default();
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "1.0.0".to_string(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        assert!(init.capabilities.tools.is_none());
        let resources_cap = init.capabilities.resources.unwrap();
        assert_eq!(resources_cap.subscribe, Some(false));
        assert_eq!(resources_cap.list_changed, Some(true));

        let resources = server.list_resources(ctx.ctx(), None).await.unwrap();
        assert_eq!(resources.resources.len(), 1);
        assert_eq!(resources.resources[0].uri, "tmcp://api/echo.d.luau");

        let templates = server
            .list_resource_templates(ctx.ctx(), None)
            .await
            .unwrap();
        assert_eq!(templates.resource_templates.len(), 1);
        assert_eq!(
            templates.resource_templates[0].uri_template,
            "tmcp://api/{tool}.d.luau"
        );

        let doc = server
            .read_resource(ctx.ctx(), "tmcp://api/echo.d.luau".to_string())
            .await
            .unwrap();
        match &doc.contents[0] {
            ResourceContents::Text(text) => {
                assert_eq!(text.text, "declare function echo(message: string): string");
            }
            _ => panic!("Expected text resource"),
        }

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert!(tools.tools.is_empty());
    }
}
