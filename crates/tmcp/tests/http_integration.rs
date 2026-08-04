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
        Arguments, Client, ClientCtx, ClientHandler, Result, Server, ServerCtx, ServerHandler,
        ToolError,
        schema::{self, *},
    };
    use tokio::{
        net::TcpListener,
        sync::mpsc,
        time::{Duration, sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;
    use tracing_subscriber::fmt;

    #[derive(Clone)]
    struct InjectedExtension(&'static str);

    #[derive(Default)]
    struct EchoConnection;

    fn versions(values: &[&str]) -> SupportedProtocolVersions {
        SupportedProtocolVersions::new(values.iter().map(|value| value.parse().unwrap())).unwrap()
    }

    #[async_trait]
    impl ServerHandler for EchoConnection {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
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
        ) -> Result<CallToolResponse> {
            match name.as_str() {
                "echo" => {
                    let Some(args) = arguments else {
                        let result: CallToolResult =
                            ToolError::invalid_input("Missing args").into();
                        return Ok(CallToolResponse::result(result));
                    };
                    let Some(message) = args.get::<String>("message") else {
                        let result: CallToolResult =
                            ToolError::invalid_input("Missing message").into();
                        return Ok(CallToolResponse::result(result));
                    };
                    Ok(CallToolResponse::result(
                        CallToolResult::new().with_text_content(message),
                    ))
                }
                "extension_echo" => {
                    let extension = context
                        .extensions()
                        .get::<InjectedExtension>()
                        .map(|value| value.0)
                        .unwrap_or("missing");
                    Ok(CallToolResponse::result(
                        CallToolResult::new().with_text_content(extension),
                    ))
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
                "protocolVersion": "2025-11-25",
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
            .with_protocol_versions(versions(&["2025-06-18"]))
            .serve_http("127.0.0.1:0")
            .await
            .unwrap();

        let bound_addr = server_handle.bound_addr.as_ref().unwrap();
        sleep(Duration::from_millis(100)).await;

        let mut client = Client::new("http-test-client", "0.1.0")
            .with_protocol_versions(versions(&["2025-11-25", "2025-06-18"]));
        let init = client
            .connect_http(&format!("http://{bound_addr}"))
            .await
            .unwrap();
        assert_eq!(init.server_info.name, "http-echo-server");
        assert_eq!(init.protocol_version.as_str(), "2025-06-18");

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
            .header("MCP-Protocol-Version", "2025-11-25")
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

    #[tokio::test]
    async fn test_http_builder_endpoint_addr_includes_endpoint_path() {
        let server_handle = Server::new(EchoConnection::default)
            .http("127.0.0.1:0")
            .with_endpoint_path("/mcp")
            .serve()
            .await
            .unwrap();

        let endpoint_addr = server_handle.endpoint_addr().unwrap();
        assert!(endpoint_addr.ends_with("/mcp"));

        let mut client = Client::new("http-test-client", "0.1.0");
        let init = client
            .connect_http(format!("http://{endpoint_addr}"))
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
    async fn test_http_embed_default_root_router_merges() {
        let embedded = Server::new(EchoConnection::default)
            .http_embed()
            .into_router()
            .await
            .unwrap();
        let tmcp::EmbeddedHttpServer { router, handle } = embedded;

        let shutdown = CancellationToken::new();
        let shutdown_task = shutdown.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .merge(router);
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_task.cancelled().await;
                })
                .await
                .unwrap();
        });

        let mut client = Client::new("http-test-client", "0.1.0");
        let init = client.connect_http(format!("http://{addr}")).await.unwrap();
        assert_eq!(init.server_info.name, "http-echo-server");

        let health = HttpClient::new()
            .get(format!("http://{addr}/healthz"))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        drop(client);
        shutdown.cancel();
        server_task.await.unwrap();
        handle.stop().await.unwrap();
    }

    /// Server whose initialize advertises tool list-changed notifications.
    #[derive(Default)]
    struct NotifyServer;

    #[async_trait]
    impl ServerHandler for NotifyServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("notify-server")
                .with_version("0.1.0")
                .with_capabilities(ServerCapabilities::default().with_tools(Some(true))))
        }
    }

    /// Client handler that forwards server notifications to a channel.
    #[derive(Clone)]
    struct NotificationCapture {
        tx: mpsc::Sender<ServerNotification>,
    }

    #[async_trait]
    impl ClientHandler for NotificationCapture {
        async fn notification(
            &self,
            _context: &ClientCtx,
            notification: ServerNotification,
        ) -> Result<()> {
            self.tx.send(notification).await.ok();
            Ok(())
        }
    }

    #[tokio::test]
    async fn server_notifications_reach_http_clients_over_sse() {
        let server = Server::new(NotifyServer::default);
        let handle = server.serve_http("127.0.0.1:0").await.unwrap();
        let addr = handle.bound_addr.clone().unwrap();

        let (tx, mut rx) = mpsc::channel(8);
        let mut client =
            Client::new("sse-client", "1.0.0").with_handler(NotificationCapture { tx });
        client
            .connect_http(&format!("http://{addr}"))
            .await
            .unwrap();

        // The client's SSE stream attaches asynchronously after initialize,
        // so retry the send until a notification is observed.
        let received = timeout(Duration::from_secs(10), async {
            loop {
                handle.send_server_notification(&ServerNotification::tool_list_changed());
                if let Ok(Some(notification)) = timeout(Duration::from_millis(100), rx.recv()).await
                {
                    return notification;
                }
            }
        })
        .await
        .expect("notification was never delivered over SSE");

        assert!(matches!(
            received,
            ServerNotification::ToolListChanged { .. }
        ));

        drop(client);
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn session_lifecycle_over_raw_http() {
        let server =
            Server::new(EchoConnection::default).with_protocol_versions(versions(&["2025-06-18"]));
        let handle = server.serve_http("127.0.0.1:0").await.unwrap();
        let addr = handle.bound_addr.clone().unwrap();
        let base = format!("http://{addr}");
        let http = HttpClient::new();

        // An unsupported request falls back to the server's latest version.
        let response = http
            .post(&base)
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&initialize_payload())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["MCP-Protocol-Version"], "2025-06-18");
        let session_id = response
            .headers()
            .get("Mcp-Session-Id")
            .expect("session id header")
            .to_str()
            .unwrap()
            .to_string();
        let initialize_response: serde_json::Value = response.json().await.unwrap();
        assert_eq!(
            initialize_response["result"]["protocolVersion"],
            "2025-06-18"
        );

        // A version that does not match the session is rejected.
        let response = http
            .post(&base)
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "1999-01-01")
            .header("Mcp-Session-Id", &session_id)
            .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // GET with an Accept header that excludes SSE is rejected.
        let response = http
            .get(&base)
            .header("Accept", "application/json")
            .header("Mcp-Session-Id", &session_id)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);

        // DELETE terminates the session.
        let response = http
            .delete(&base)
            .header("Mcp-Session-Id", &session_id)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Requests against the terminated session no longer resolve.
        let response = http
            .post(&base)
            .header("Content-Type", "application/json")
            .header("Mcp-Session-Id", &session_id)
            .json(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_clients_with_colliding_request_ids() {
        let server = Server::new(EchoConnection::default);
        let handle = server.serve_http("127.0.0.1:0").await.unwrap();
        let addr = handle.bound_addr.clone().unwrap();

        // Two independent clients both number their JSON-RPC requests from
        // the same starting id, so their ids collide on the wire. Each
        // session must still receive exactly its own responses.
        let mut client1 = Client::new("collide-1", "1.0.0");
        client1
            .connect_http(&format!("http://{addr}"))
            .await
            .unwrap();
        let mut client2 = Client::new("collide-2", "1.0.0");
        client2
            .connect_http(&format!("http://{addr}"))
            .await
            .unwrap();

        for round in 0..5 {
            let message1 = format!("client1-round{round}");
            let message2 = format!("client2-round{round}");
            let (result1, result2) = tokio::join!(
                client1.call_tool("echo", json!({ "message": message1.clone() })),
                client2.call_tool("echo", json!({ "message": message2.clone() })),
            );
            let result1 = result1.unwrap();
            let result2 = result2.unwrap();
            let text1 = match &result1.content[0] {
                ContentBlock::Text(text) => text.text.clone(),
                other => panic!("unexpected content: {other:?}"),
            };
            let text2 = match &result2.content[0] {
                ContentBlock::Text(text) => text.text.clone(),
                other => panic!("unexpected content: {other:?}"),
            };
            assert_eq!(text1, message1, "client1 received the wrong response");
            assert_eq!(text2, message2, "client2 received the wrong response");
        }

        drop(client1);
        drop(client2);
        handle.stop().await.unwrap();
    }
}
