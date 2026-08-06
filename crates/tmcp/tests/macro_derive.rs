//! Macro derive tests for the tmcp procedural macros.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, marker::PhantomData};

    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use tmcp::{
        Error, Result, ServerCtx, ServerHandler, ToolGroup, ToolResponse, ToolResult, ToolSet,
        delegate_server_handler, mcp_server, schema::*, testutils::TestServerContext, tool,
        tool_group,
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

    struct DelegatedState(String);

    #[tool(defaults, read_only)]
    /// Return a message through delegated state.
    async fn delegated_echo(
        state: &DelegatedState,
        _ctx: &ServerCtx,
        _task: Option<TaskMetadata>,
        params: EchoParams,
    ) -> ToolResult<PingResponse> {
        Ok(PingResponse {
            message: format!("{}{}", state.0, params.message),
        })
    }

    #[tool(defaults)]
    /// Return an upper-case message through delegated state.
    async fn delegated_upper(
        state: &DelegatedState,
        params: EchoParams,
    ) -> ToolResult<PingResponse> {
        Ok(PingResponse {
            message: format!("{}{}", state.0, params.message.to_uppercase()),
        })
    }

    #[tool_group(
        state = DelegatedState,
        tools = [delegated_echo, delegated_upper]
    )]
    struct DelegatedTools;

    #[tool_group(state = DelegatedState, tools = [delegated_echo])]
    struct DuplicateDelegatedTools;

    #[derive(Debug)]
    struct GroupDelegatingServer {
        prefix: String,
    }

    #[mcp_server(
        tool_groups = [DelegatedTools],
        tool_state_fn = delegated_state,
        tool_state_param = (tenant_id: String, "Tenant identifier.")
    )]
    impl GroupDelegatingServer {
        async fn delegated_state(
            &self,
            arguments: Option<&tmcp::Arguments>,
        ) -> ToolResult<DelegatedState> {
            if arguments
                .and_then(|args| args.get::<String>("tenant_id"))
                .as_deref()
                != Some("acme")
            {
                return Err(tmcp::ToolError::invalid_input("invalid tenant id"));
            }
            Ok(DelegatedState(self.prefix.clone()))
        }
    }

    #[derive(Debug)]
    struct DuplicateGroupServer;

    #[mcp_server(
        tool_groups = [DelegatedTools, DuplicateDelegatedTools],
        tool_state_fn = delegated_state
    )]
    impl DuplicateGroupServer {
        async fn delegated_state(
            &self,
            _arguments: Option<&tmcp::Arguments>,
        ) -> ToolResult<DelegatedState> {
            Ok(DelegatedState(String::new()))
        }
    }

    #[derive(Default)]
    struct ToolsetGroupServer {
        tools: ToolSet,
    }

    #[mcp_server(
        toolset = "tools",
        tool_groups = [DelegatedTools],
        tool_state_fn = delegated_state,
        tool_state_param = (tenant_id: String, "Tenant identifier.")
    )]
    impl ToolsetGroupServer {
        async fn delegated_state(
            &self,
            arguments: Option<&tmcp::Arguments>,
        ) -> ToolResult<DelegatedState> {
            if arguments
                .and_then(|args| args.get::<String>("tenant_id"))
                .as_deref()
                != Some("acme")
            {
                return Err(tmcp::ToolError::invalid_input("invalid tenant id"));
            }
            Ok(DelegatedState("toolset:".to_string()))
        }
    }

    #[derive(Debug)]
    struct DelegatingServer {
        prefix: String,
    }

    #[mcp_server(tools = [delegated_echo], tool_state_fn = delegated_state)]
    impl DelegatingServer {
        async fn delegated_state(
            &self,
            arguments: Option<&tmcp::Arguments>,
        ) -> ToolResult<DelegatedState> {
            if arguments
                .and_then(|args| args.get::<String>("message"))
                .as_deref()
                == Some("blocked")
            {
                return Err(tmcp::ToolError::invalid_input("message is blocked"));
            }
            Ok(DelegatedState(self.prefix.clone()))
        }
    }

    #[tokio::test]
    async fn delegated_tools_preserve_schema_tasks_and_dispatch() {
        let server = DelegatingServer {
            prefix: "state:".to_string(),
        };
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert!(init.capabilities.tasks.is_some());

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert_eq!(tools.tools.len(), 1);
        let tool = &tools.tools[0];
        assert_eq!(tool.name, "delegated_echo");
        assert_eq!(
            tool.description.as_deref(),
            Some("Return a message through delegated state.")
        );
        assert_eq!(
            tool.input_schema.0["required"],
            serde_json::json!(["message"])
        );
        assert!(tool.output_schema.is_some());
        assert!(matches!(
            tool.execution
                .as_ref()
                .and_then(|execution| execution.task_support.as_ref()),
            Some(ToolTaskSupport::Optional)
        ));

        let result = server
            .call_tool(
                ctx.ctx(),
                "delegated_echo".to_string(),
                Some(tmcp::Arguments::new().insert("message", "hello")),
                None,
            )
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "message": "state:hello" }))
        );

        let result = server
            .call_tool(
                ctx.ctx(),
                "delegated_echo".to_string(),
                Some(tmcp::Arguments::new().insert("message", "blocked")),
                None,
            )
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn delegated_tool_group_owns_names_schemas_and_dispatch() {
        let server = GroupDelegatingServer {
            prefix: "group:".to_string(),
        };
        let ctx = TestServerContext::new();

        assert_eq!(
            <DelegatedTools as ToolGroup>::NAMES,
            ["delegated_echo", "delegated_upper"]
        );
        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert!(init.capabilities.tasks.is_some());
        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert_eq!(tools.tools.len(), 2);
        assert!(tools.tools.iter().all(|tool| {
            tool.input_schema.required() == Some(vec!["message", "tenant_id"])
                && tool.input_schema.properties().is_some_and(|properties| {
                    properties["tenant_id"]["description"] == "Tenant identifier."
                })
        }));
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| { <DelegatedTools as ToolGroup>::NAMES.contains(&tool.name.as_str()) })
        );

        let result = server
            .call_tool(
                ctx.ctx(),
                "delegated_upper".to_string(),
                Some(
                    tmcp::Arguments::new()
                        .insert("message", "hello")
                        .insert("tenant_id", "acme"),
                ),
                None,
            )
            .await
            .unwrap()
            .into_result()
            .expect("immediate group tool response");
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "message": "group:HELLO" }))
        );
    }

    #[tokio::test]
    async fn delegated_tool_groups_reject_duplicate_names() {
        let ctx = TestServerContext::new();

        let error = DuplicateGroupServer
            .list_tools(ctx.ctx(), None)
            .await
            .expect_err("duplicate group names must fail");

        assert!(matches!(
            error,
            Error::InvalidConfiguration(message)
                if message == "duplicate tool name `delegated_echo`"
        ));
    }

    #[tokio::test]
    async fn toolset_server_registers_delegated_tool_group() {
        let ctx = TestServerContext::new();
        let server = ToolsetGroupServer::default();

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert_eq!(tools.tools.len(), 2);
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| tool.input_schema.is_required("tenant_id"))
        );
        let result = server
            .call_tool(
                ctx.ctx(),
                "delegated_echo".to_string(),
                Some(
                    tmcp::Arguments::new()
                        .insert("message", "hello")
                        .insert("tenant_id", "acme"),
                ),
                None,
            )
            .await
            .unwrap()
            .into_result()
            .expect("immediate toolset group response");
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({ "message": "toolset:hello" }))
        );
    }

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

    struct DelegatingHandler {
        inner: TestServer,
    }

    #[delegate_server_handler(self.inner)]
    #[async_trait::async_trait]
    impl ServerHandler for DelegatingHandler {
        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            Ok(ListToolsResult::new())
        }
    }

    #[tokio::test]
    async fn delegated_handler_forwards_omitted_methods_and_keeps_overrides() {
        let server = DelegatingHandler { inner: TestServer };
        let ctx = TestServerContext::new();
        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "test_server");
        assert!(
            server
                .list_tools(ctx.ctx(), None)
                .await
                .unwrap()
                .tools
                .is_empty()
        );
        let response = server
            .handle_request(ctx.ctx(), ClientRequest::list_tools(None))
            .await
            .unwrap();
        let tools: ListToolsResult = serde_json::from_value(response).unwrap();
        assert!(tools.tools.is_empty());
        server.pong(ctx.ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn test_initialize() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        let result = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        assert_eq!(TestServer::NAMES, ["echo", "add", "multiply", "ping"]);

        assert_eq!(result.server_info.name, "test_server");
        assert_eq!(
            result.instructions,
            Some("Test server with echo and add tools".to_string())
        );
        let tools_cap = result.capabilities.tools.expect("tools capability");
        assert_eq!(tools_cap.list_changed, None);
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
            .unwrap()
            .into_result()
            .expect("immediate tool response");
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
            .unwrap()
            .into_result()
            .expect("immediate tool response");
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
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "12"),
            _ => panic!("Expected text content"),
        }

        let result = server
            .call_tool(ctx.ctx(), "ping".to_string(), None, None)
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
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
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        assert_eq!(result.is_error, Some(true));

        // Invalid arguments
        let mut args = HashMap::new();
        args.insert("a".to_string(), serde_json::json!("not a number"));
        args.insert("b".to_string(), serde_json::json!(2.0));

        let result = server
            .call_tool(ctx.ctx(), "add".to_string(), Some(args.into()), None)
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
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
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("custom_init_server")
                .with_version("2.0.0")
                .with_tools(Some(true))
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
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        // Verify custom initialization was used
        assert_eq!(result.server_info.name, "custom_init_server");
        assert_eq!(result.server_info.version, "2.0.0");
        assert_eq!(result.protocol_version.as_str(), "2025-11-25");
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
            .unwrap()
            .into_result()
            .expect("immediate tool response");

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
                "2025-11-25".parse().unwrap(),
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

    #[derive(Debug, Default)]
    struct FlatAttrServer;

    #[mcp_server]
    /// Server exercising flat-parameter attributes
    impl FlatAttrServer {
        #[tool]
        /// Repeat a label
        async fn label(
            &self,
            /// The label text
            text: String,
            /// Number of repetitions
            #[serde(default)]
            count: i64,
        ) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(format!("{text}:{count}")))
        }
    }

    #[tokio::test]
    async fn test_flat_param_attrs_reach_schema() {
        let server = FlatAttrServer;
        let ctx = TestServerContext::new();

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        let tool = tools.tools.iter().find(|t| t.name == "label").unwrap();
        let properties = &tool.input_schema.0["properties"];
        assert_eq!(
            properties["text"]["description"],
            serde_json::json!("The label text")
        );
        assert_eq!(
            properties["count"]["description"],
            serde_json::json!("Number of repetitions")
        );
    }

    #[tokio::test]
    async fn test_flat_param_serde_default() {
        let server = FlatAttrServer;
        let ctx = TestServerContext::new();

        let mut args = HashMap::new();
        args.insert("text".to_string(), serde_json::json!("x"));

        let result = server
            .call_tool(ctx.ctx(), "label".to_string(), Some(args.into()), None)
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "x:0"),
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_task_rejected_for_non_task_tool() {
        let server = TestServer;
        let ctx = TestServerContext::new();

        let err = server
            .call_tool(
                ctx.ctx(),
                "ping".to_string(),
                None,
                Some(TaskMetadata { ttl: None }),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidParams(_)));
        assert!(err.to_string().contains("task-augmented"));
    }

    mod inner {
        /// Server type defined behind a module path.
        #[derive(Debug, Default)]
        pub struct PathServer;
    }

    #[mcp_server]
    /// Path-qualified server
    impl inner::PathServer {
        #[tool]
        /// Say hello
        async fn hello(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content("hello"))
        }

        #[tool]
        /// Move something
        async fn r#move(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content("moved"))
        }
    }

    #[tokio::test]
    async fn test_path_qualified_impl() {
        let server = inner::PathServer;
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "path_server");

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert!(tools.tools.iter().any(|t| t.name == "hello"));
        assert!(tools.tools.iter().any(|t| t.name == "move"));

        let result = server
            .call_tool(ctx.ctx(), "move".to_string(), None, None)
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "moved"),
            _ => panic!("Expected text content"),
        }
    }

    #[derive(Debug, Default)]
    struct UndocumentedServer;

    #[mcp_server]
    impl UndocumentedServer {
        #[tool]
        async fn noop(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new())
        }
    }

    #[tokio::test]
    async fn test_empty_description_no_instructions() {
        let server = UndocumentedServer;
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert_eq!(init.instructions, None);
    }

    #[derive(Debug, Default)]
    struct ParagraphServer;

    #[mcp_server]
    /// First paragraph.
    ///
    /// Second paragraph.
    impl ParagraphServer {
        #[tool]
        /// Tool summary.
        ///
        /// Tool details.
        async fn noop(&self) -> Result<CallToolResult> {
            Ok(CallToolResult::new())
        }
    }

    #[tokio::test]
    async fn test_doc_paragraphs_preserved() {
        let server = ParagraphServer;
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert_eq!(
            init.instructions,
            Some("First paragraph.\n\nSecond paragraph.".to_string())
        );

        let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
        assert_eq!(
            tools.tools[0].description,
            Some("Tool summary.\n\nTool details.".to_string())
        );
    }

    #[derive(Debug, Default)]
    struct GenericServer<T> {
        marker: PhantomData<T>,
    }

    #[mcp_server]
    /// Generic server
    impl<T: Send + Sync + 'static> GenericServer<T> {
        #[tool]
        /// Echo the message
        async fn echo(&self, params: EchoParams) -> Result<CallToolResult> {
            Ok(CallToolResult::new().with_text_content(params.message))
        }
    }

    #[tokio::test]
    async fn test_generic_impl() {
        let server = GenericServer::<u8>::default();
        let ctx = TestServerContext::new();

        let init = server
            .initialize(
                ctx.ctx(),
                "2025-11-25".parse().unwrap(),
                ClientCapabilities::default(),
                Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "generic_server");

        let mut args = HashMap::new();
        args.insert("message".to_string(), serde_json::json!("hi"));

        let result = server
            .call_tool(ctx.ctx(), "echo".to_string(), Some(args.into()), None)
            .await
            .unwrap()
            .into_result()
            .expect("immediate tool response");
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "hi"),
            _ => panic!("Expected text content"),
        }
    }
}
