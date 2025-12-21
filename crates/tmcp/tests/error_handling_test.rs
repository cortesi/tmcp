//! Unit tests for JSON-RPC error handling
//!
//! These tests focus on testing error handling at the Connection level
//! without the complexity of setting up full client-server communication.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tmcp::{Arguments, Error, Result, ServerConn, ServerCtx, schema, testutils};
    use tokio::sync::broadcast;

    fn create_test_context() -> ServerCtx {
        let (notification_tx, _) = broadcast::channel(100);
        testutils::test_server_ctx(notification_tx)
    }

    #[tokio::test]
    async fn test_method_not_found() {
        // Test that the default tools_call implementation returns ToolNotFound
        #[derive(Default)]
        struct MinimalConnection;

        #[async_trait::async_trait]
        impl ServerConn for MinimalConnection {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: schema::ClientCapabilities,
                _client_info: schema::Implementation,
            ) -> Result<schema::InitializeResult> {
                Ok(schema::InitializeResult::new("test-server").with_version("1.0.0"))
            }
        }

        let conn = MinimalConnection;

        // Call a tool on a connection that doesn't implement tools_call
        // This should use the default implementation which returns ToolNotFound
        let context = create_test_context();
        let result = conn
            .call_tool(&context, "non_existent".to_string(), None)
            .await;

        assert!(result.is_err());
        match result {
            Err(Error::ToolExecutionFailed { tool, message }) => {
                assert_eq!(tool, "non_existent");
                assert!(message.contains("not found"), "Message was: {message}");
            }
            _ => panic!("unexpected result: {result:?}"),
        }
    }

    #[tokio::test]
    async fn test_invalid_params() {
        // Test parameter validation in tools_call
        #[derive(Default)]
        struct ConnectionWithValidation;

        #[async_trait::async_trait]
        impl ServerConn for ConnectionWithValidation {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: schema::ClientCapabilities,
                _client_info: schema::Implementation,
            ) -> Result<schema::InitializeResult> {
                Ok(schema::InitializeResult::new("test-server")
                    .with_version("1.0.0")
                    .with_tools(false))
            }

            async fn list_tools(
                &self,
                _context: &ServerCtx,
                _cursor: Option<schema::Cursor>,
            ) -> Result<schema::ListToolsResult> {
                let schema = schema::ToolSchema {
                    schema_type: "object".to_string(),
                    properties: Some({
                        let mut props = HashMap::new();
                        props.insert(
                            "required_param".to_string(),
                            serde_json::json!({
                                "type": "string",
                                "description": "A required parameter"
                            }),
                        );
                        props
                    }),
                    required: Some(vec!["required_param".to_string()]),
                };

                Ok(schema::ListToolsResult::new().with_tool(
                    schema::Tool::new("test_tool", schema)
                        .with_description("A test tool that requires a parameter"),
                ))
            }

            async fn call_tool(
                &self,
                _context: &ServerCtx,
                name: String,
                arguments: Option<Arguments>,
            ) -> Result<schema::CallToolResult> {
                if name != "test_tool" {
                    return Err(Error::ToolNotFound(name));
                }

                // Validate arguments
                let args = arguments
                    .ok_or_else(|| Error::InvalidParams("Missing arguments".to_string()))?;

                if args.get_value("required_param").is_none() {
                    return Err(Error::InvalidParams("Missing required_param".to_string()));
                }

                Ok(schema::CallToolResult::new()
                    .with_text_content("Success")
                    .is_error(false))
            }
        }

        let conn = ConnectionWithValidation;

        // Test 1: Call with missing arguments
        let context = create_test_context();
        let result = conn
            .call_tool(&context, "test_tool".to_string(), None)
            .await;
        assert!(matches!(result, Err(Error::InvalidParams(_))));

        // Test 2: Call with empty object (missing required param)
        let context = create_test_context();
        let result = conn
            .call_tool(
                &context,
                "test_tool".to_string(),
                Some(HashMap::<String, serde_json::Value>::new().into()),
            )
            .await;
        match result {
            Err(Error::InvalidParams(msg)) => {
                assert!(msg.contains("required_param"), "Error was: {msg}");
            }
            _ => panic!("Expected InvalidParams error"),
        }

        // Test 3: Call with correct parameters should succeed
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("required_param".to_string(), serde_json::json!("test"));
        let result = conn
            .call_tool(&context, "test_tool".to_string(), Some(args.into()))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_successful_response() {
        // Test successful tool listing and other operations
        #[derive(Default)]
        struct ConnectionWithTools;

        #[async_trait::async_trait]
        impl ServerConn for ConnectionWithTools {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: schema::ClientCapabilities,
                _client_info: schema::Implementation,
            ) -> Result<schema::InitializeResult> {
                Ok(schema::InitializeResult::new("test-server")
                    .with_version("1.0.0")
                    .with_tools(false)
                    .with_resources(true, false))
            }

            async fn list_tools(
                &self,
                _context: &ServerCtx,
                _cursor: Option<schema::Cursor>,
            ) -> Result<schema::ListToolsResult> {
                Ok(schema::ListToolsResult::new()
                    .with_tool(
                        schema::Tool::new("echo", schema::ToolSchema::default())
                            .with_description("Echoes the input"),
                    )
                    .with_tool(
                        schema::Tool::new("add", schema::ToolSchema::default())
                            .with_description("Adds two numbers"),
                    ))
            }

            async fn list_resources(
                &self,
                _context: &ServerCtx,
                _cursor: Option<schema::Cursor>,
            ) -> Result<schema::ListResourcesResult> {
                Ok(
                    schema::ListResourcesResult::new().with_resource(schema::Resource {
                        uri: "file:///test.txt".to_string(),
                        name: "test.txt".to_string(),
                        title: None,
                        description: Some("A test file".to_string()),
                        mime_type: Some("text/plain".to_string()),
                        size: None,
                        annotations: None,
                        _meta: None,
                    }),
                )
            }
        }

        let conn = ConnectionWithTools;

        // Test successful initialization
        let context = create_test_context();
        let init_result = conn
            .initialize(
                &context,
                schema::LATEST_PROTOCOL_VERSION.to_string(),
                schema::ClientCapabilities::default(),
                schema::Implementation::new("test-client", "1.0.0"),
            )
            .await
            .unwrap();

        assert_eq!(init_result.server_info.name, "test-server");
        assert!(init_result.capabilities.tools.is_some());
        assert!(init_result.capabilities.resources.is_some());

        // Test successful tools listing
        let context = create_test_context();
        let tools = conn.list_tools(&context, None).await.unwrap();
        assert_eq!(tools.tools.len(), 2);
        assert_eq!(tools.tools[0].name, "echo");
        assert_eq!(tools.tools[1].name, "add");

        // Test successful resources listing
        let context = create_test_context();
        let resources = conn.list_resources(&context, None).await.unwrap();
        assert_eq!(resources.resources.len(), 1);
        assert_eq!(resources.resources[0].uri, "file:///test.txt");
    }

    #[tokio::test]
    async fn test_error_propagation() {
        // Test that errors are properly propagated through the Connection trait
        #[derive(Default)]
        struct FaultyConnection;

        #[async_trait::async_trait]
        impl ServerConn for FaultyConnection {
            async fn initialize(
                &self,
                _context: &ServerCtx,
                _protocol_version: String,
                _capabilities: schema::ClientCapabilities,
                _client_info: schema::Implementation,
            ) -> Result<schema::InitializeResult> {
                // Simulate an internal error during initialization
                Err(Error::InternalError("Connection failed".to_string()))
            }

            async fn read_resource(
                &self,
                _context: &ServerCtx,
                uri: String,
            ) -> Result<schema::ReadResourceResult> {
                // Simulate resource not found
                Err(Error::ResourceNotFound { uri })
            }

            async fn get_prompt(
                &self,
                _context: &ServerCtx,
                name: String,
                _arguments: Option<Arguments>,
            ) -> Result<schema::GetPromptResult> {
                // Simulate prompt not found - using MethodNotFound as PromptNotFound doesn't exist
                Err(Error::MethodNotFound(format!("prompt/{name}")))
            }
        }

        let conn = FaultyConnection;

        // Test initialization error
        let context = create_test_context();
        let init_result = conn
            .initialize(
                &context,
                schema::LATEST_PROTOCOL_VERSION.to_string(),
                schema::ClientCapabilities::default(),
                schema::Implementation::new("test-client", "1.0.0"),
            )
            .await;

        match init_result {
            Err(Error::InternalError(msg)) => {
                assert!(msg.contains("Connection failed"));
            }
            _ => panic!("Expected InternalError"),
        }

        // Test resource not found
        let context = create_test_context();
        let read_result = conn
            .read_resource(&context, "file:///missing.txt".to_string())
            .await;
        match read_result {
            Err(Error::ResourceNotFound { uri }) => {
                assert_eq!(uri, "file:///missing.txt");
            }
            _ => panic!("Expected ResourceNotFound error"),
        }

        // Test prompt not found (using MethodNotFound)
        let context = create_test_context();
        let prompt_result = conn
            .get_prompt(&context, "missing_prompt".to_string(), None)
            .await;
        match prompt_result {
            Err(Error::MethodNotFound(method)) => {
                assert!(method.contains("missing_prompt"));
            }
            _ => panic!("Expected MethodNotFound error"),
        }
    }
}
