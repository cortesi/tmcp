//! MCP Protocol Compliance Tests
//!
//! This module contains tests that verify our implementation follows the MCP
//! specification correctly. These tests focus on protocol compliance without
//! requiring external dependencies.
//!
//! For actual interoperability tests with rmcp, see rmcp_integration.rs

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::json;
use tmcp::{Arguments, Error, Result, ServerCtx, ServerHandler, ToolError, schema::*, testutils};

/// Test connection implementation with echo and add tools
struct TestConnection {
    tools: HashMap<String, Tool>,
}

impl TestConnection {
    fn new() -> Self {
        let mut tools = HashMap::new();

        // Echo tool
        let echo_schema = ToolSchema::default()
            .with_property(
                "message",
                json!({
                    "type": "string",
                    "description": "The message to echo"
                }),
            )
            .with_required("message");

        tools.insert(
            "echo".to_string(),
            Tool::new("echo", echo_schema).with_description("Echoes the input message"),
        );

        // Add tool
        let add_schema = ToolSchema::default()
            .with_property(
                "a",
                json!({
                    "type": "number",
                    "description": "First number"
                }),
            )
            .with_property(
                "b",
                json!({
                    "type": "number",
                    "description": "Second number"
                }),
            )
            .with_required("a")
            .with_required("b");

        tools.insert(
            "add".to_string(),
            Tool::new("add", add_schema).with_description("Adds two numbers"),
        );

        Self { tools }
    }
}

#[async_trait]
impl ServerHandler for TestConnection {
    async fn initialize(
        &self,
        _context: &ServerCtx,
        _protocol_version: String,
        _capabilities: ClientCapabilities,
        _client_info: Implementation,
    ) -> Result<InitializeResult> {
        Ok(InitializeResult::new("test-server")
            .with_version("1.0.0")
            .with_tools(true))
    }

    async fn list_tools(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        let mut result = ListToolsResult::new();
        for tool in self.tools.values() {
            result = result.with_tool(tool.clone());
        }
        Ok(result)
    }

    async fn call_tool(
        &self,
        _context: &ServerCtx,
        name: String,
        arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        match name.as_str() {
            "echo" => {
                let Some(args) = arguments else {
                    return Ok(ToolError::invalid_input("echo: Missing arguments").into());
                };
                let Some(message) = args.get_string("message") else {
                    return Ok(ToolError::invalid_input("echo: Missing message parameter").into());
                };

                Ok(CallToolResult::new().with_text_content(message))
            }
            "add" => {
                let Some(args) = arguments else {
                    return Ok(ToolError::invalid_input("add: Missing arguments").into());
                };

                let Some(a) = args.get::<f64>("a") else {
                    return Ok(
                        ToolError::invalid_input("add: Missing or invalid 'a' parameter").into(),
                    );
                };
                let Some(b) = args.get::<f64>("b") else {
                    return Ok(
                        ToolError::invalid_input("add: Missing or invalid 'b' parameter").into(),
                    );
                };

                Ok(CallToolResult::new().with_text_content(format!("{}", a + b)))
            }
            _ => Err(Error::ToolExecutionFailed {
                tool: name,
                message: "Tool not found".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;

    fn create_test_context() -> ServerCtx {
        let (notification_tx, _) = broadcast::channel(100);
        testutils::test_server_ctx(notification_tx)
    }

    #[tokio::test]
    async fn test_echo_tool() {
        let conn = TestConnection::new();

        // Test tools list
        let context = create_test_context();
        let tools_result = conn.list_tools(&context, None).await.unwrap();
        assert_eq!(tools_result.tools.len(), 2);
        let echo_tool = tools_result
            .tools
            .iter()
            .find(|t| t.name == "echo")
            .unwrap();
        assert_eq!(echo_tool.name, "echo");
        assert!(echo_tool.description.is_some());

        // Test execution
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("message".to_string(), json!("Hello, World!"));
        let result = conn
            .call_tool(&context, "echo".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "Hello, World!"),
            _ => panic!("Expected text content"),
        }

        // Test error on missing arguments
        let context = create_test_context();
        let error = conn
            .call_tool(&context, "echo".to_string(), None, None)
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));

        // Test error on missing message field
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("wrong_field".to_string(), json!("value"));
        let error = conn
            .call_tool(&context, "echo".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_add_tool() {
        let conn = TestConnection::new();

        // Test tools list contains add tool
        let context = create_test_context();
        let tools_result = conn.list_tools(&context, None).await.unwrap();
        let add_tool = tools_result.tools.iter().find(|t| t.name == "add").unwrap();
        assert_eq!(add_tool.name, "add");
        assert!(add_tool.description.is_some());

        // Test integer addition
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("a".to_string(), json!(5));
        args.insert("b".to_string(), json!(3));
        let result = conn
            .call_tool(&context, "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "8"),
            _ => panic!("Expected text content"),
        }

        // Test float addition
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("a".to_string(), json!(1.5));
        args.insert("b".to_string(), json!(2.5));
        let result = conn
            .call_tool(&context, "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "4"),
            _ => panic!("Expected text content"),
        }

        // Test negative numbers
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("a".to_string(), json!(-5));
        args.insert("b".to_string(), json!(3));
        let result = conn
            .call_tool(&context, "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text(text) => assert_eq!(text.text, "-2"),
            _ => panic!("Expected text content"),
        }

        // Test error on missing arguments
        let context = create_test_context();
        let error = conn
            .call_tool(&context, "add".to_string(), None, None)
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));

        // Test error on missing 'a' field
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("b".to_string(), json!(5));
        let error = conn
            .call_tool(&context, "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));

        // Test error on missing 'b' field
        let context = create_test_context();
        let mut args = HashMap::new();
        args.insert("a".to_string(), json!(5));
        let error = conn
            .call_tool(&context, "add".to_string(), Some(args.into()), None)
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));
    }

    #[tokio::test]
    async fn test_protocol_compliance() {
        // Test that our tools follow the MCP protocol specification
        let conn = TestConnection::new();
        let context = create_test_context();
        let tools_result = conn.list_tools(&context, None).await.unwrap();

        for tool in &tools_result.tools {
            // Verify tool has a name
            assert!(!tool.name.is_empty());

            // Verify input schema is valid
            assert_eq!(tool.input_schema.schema_type(), Some("object"));

            // Check it has properties
            let props = tool
                .input_schema
                .properties()
                .expect("should have properties");
            assert!(!props.is_empty());

            // Check it has required fields
            let required = tool.input_schema.required().expect("should have required");
            assert!(!required.is_empty());
        }
    }

    #[test]
    fn test_content_serialization() {
        // Test that Content serializes correctly
        let text_content = ContentBlock::Text(TextContent {
            text: "Hello".to_string(),
            annotations: None,
            _meta: None,
        });

        let json = serde_json::to_value(&text_content).unwrap();
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("text"));
        assert_eq!(json.get("text").and_then(|v| v.as_str()), Some("Hello"));
    }
}
