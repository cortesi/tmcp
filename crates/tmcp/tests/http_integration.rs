//! HTTP transport integration tests.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use axum::{
        Router,
        extract::Request,
        http::{HeaderName, HeaderValue, StatusCode},
        middleware::{self, Next},
        response::Response,
        routing::get,
    };
    use reqwest::Client as HttpClient;
    use serde_json::json;
    use tmcp::{
        Arguments, Client, Result, Server, ServerCtx, ServerHandler, ToolError,
        schema::{self, *},
    };
    use tokio::time::{Duration, sleep};
    use tracing_subscriber::fmt;

    #[derive(Clone)]
    struct InjectedExtension(&'static str);

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
            let echo_schema = ToolSchema::default()
                .with_property("message", json!({"type": "string"}))
                .with_required("message");
            Ok(ListToolsResult::default()
                .with_tool(Tool::new("echo", echo_schema).with_description("Echo message"))
                .with_tool(
                    Tool::new("extension_echo", ToolSchema::default())
                        .with_description("Return the injected request extension"),
                ))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResult> {
            match name.as_str() {
                "echo" => {
                    let Some(args) = arguments else {
                        return Ok(ToolError::invalid_input("Missing args").into());
                    };
                    let Some(message) = args.get::<String>("message") else {
                        return Ok(ToolError::invalid_input("Missing message").into());
                    };
                    Ok(CallToolResult::new().with_text_content(message))
                }
                "extension_echo" => {
                    let extension = context
                        .extensions()
                        .get::<InjectedExtension>()
                        .map(|value| value.0)
                        .unwrap_or("missing");
                    Ok(CallToolResult::new().with_text_content(extension))
                }
                _ => Err(tmcp::Error::ToolNotFound(name)),
            }
        }
    }

    async fn add_response_header(
        mut response: Response,
        header_name: &'static str,
        header_value: &'static str,
    ) -> Response {
        response.headers_mut().insert(
            HeaderName::from_static(header_name),
            HeaderValue::from_static(header_value),
        );
        response
    }

    async fn response_header_middleware(
        request: Request,
        next: Next,
        header_name: &'static str,
        header_value: &'static str,
    ) -> Response {
        let response = next.run(request).await;
        add_response_header(response, header_name, header_value).await
    }

    async fn extension_injection_middleware(request: Request, next: Next) -> Response {
        let mut request = request;
        request
            .extensions_mut()
            .insert(InjectedExtension("from-middleware"));
        next.run(request).await
    }

    fn initialize_payload() -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "http-test-client",
                    "version": "0.1.0"
                }
            }
        })
    }

    #[tokio::test]
    async fn test_http_echo_tool_integration() {
        fmt::try_init().ok();

        let server_handle = Server::new(EchoConnection::default)
            .serve_http("127.0.0.1:0")
            .await
            .unwrap();

        let bound_addr = server_handle.bound_addr.as_ref().unwrap();
        sleep(Duration::from_millis(100)).await;

        let mut client = Client::new("http-test-client", "0.1.0");
        let init = client
            .connect_http(&format!("http://{bound_addr}"))
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "http-echo-server");

        let mut args = HashMap::new();
        args.insert("message".to_string(), json!("hello"));
        let result = client.call_tool("echo", args).await.unwrap();
        if let Some(schema::ContentBlock::Text(text)) = result.content.first() {
            assert_eq!(text.text, "hello");
        } else {
            panic!("expected text response");
        }

        drop(client);
        server_handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_http_builder_middleware_and_routes() {
        let server_handle = Server::new(EchoConnection::default)
            .http("127.0.0.1:0")
            .with_middleware(|router| {
                router.layer(middleware::from_fn(|request, next| async move {
                    response_header_middleware(request, next, "x-layer-one", "one").await
                }))
            })
            .with_middleware(|router| {
                router.layer(middleware::from_fn(|request, next| async move {
                    response_header_middleware(request, next, "x-layer-two", "two").await
                }))
            })
            .with_routes(Router::new().route("/custom", get(|| async { StatusCode::OK })))
            .serve()
            .await
            .unwrap();

        let base_url = format!("http://{}", server_handle.bound_addr.as_ref().unwrap());
        let response = HttpClient::new()
            .post(format!("{base_url}/"))
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .json(&initialize_payload())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-layer-one"], "one");
        assert_eq!(response.headers()["x-layer-two"], "two");

        let custom = HttpClient::new()
            .get(format!("{base_url}/custom"))
            .send()
            .await
            .unwrap();
        assert_eq!(custom.status(), StatusCode::OK);
        assert!(custom.headers().get("x-layer-one").is_none());
        assert!(custom.headers().get("x-layer-two").is_none());

        server_handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_http_request_extensions_propagate_to_server_ctx() {
        let server_handle = Server::new(EchoConnection::default)
            .http("127.0.0.1:0")
            .with_middleware(|router| {
                router.layer(middleware::from_fn(extension_injection_middleware))
            })
            .serve()
            .await
            .unwrap();

        let mut client = Client::new("http-test-client", "0.1.0");
        client
            .connect_http(&format!(
                "http://{}",
                server_handle.bound_addr.as_ref().unwrap()
            ))
            .await
            .unwrap();

        let result = client
            .call_tool(
                "extension_echo",
                HashMap::<String, serde_json::Value>::new(),
            )
            .await
            .unwrap();

        if let Some(schema::ContentBlock::Text(text)) = result.content.first() {
            assert_eq!(text.text, "from-middleware");
        } else {
            panic!("expected text response");
        }

        server_handle.stop().await.unwrap();
    }
}
