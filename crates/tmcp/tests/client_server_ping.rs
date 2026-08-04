//! Client/server ping integration tests.

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tmcp::{
        ClientCtx, ClientHandler, Result, ServerCtx, ServerHandler,
        schema::*,
        testutils::{
            connected_client_and_server_with_conn, make_duplex_pair, shutdown_client_and_server,
            test_client_ctx,
        },
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        sync::mpsc,
    };
    use tracing_subscriber::fmt;

    #[derive(Default, Clone)]
    struct TestClientHandler {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ClientHandler for TestClientHandler {
        async fn on_connect(&self, _ctx: &ClientCtx) -> Result<()> {
            self.calls.lock().unwrap().push("on_connect".into());
            Ok(())
        }

        async fn on_shutdown(&self, _ctx: &ClientCtx) -> Result<()> {
            self.calls.lock().unwrap().push("on_shutdown".into());
            Ok(())
        }

        async fn pong(&self, _ctx: &ClientCtx) -> Result<()> {
            self.calls.lock().unwrap().push("ping".into());
            Ok(())
        }

        async fn create_message(
            &self,
            _ctx: &ClientCtx,
            _method: &str,
            _params: CreateMessageParams,
        ) -> Result<CreateMessageResult> {
            self.calls.lock().unwrap().push("create_message".into());
            Ok(CreateMessageResult {
                message: SamplingMessage::assistant_text("Test response"),
                model: "test-model".into(),
                stop_reason: None,
            })
        }

        async fn list_roots(&self, _ctx: &ClientCtx) -> Result<ListRootsResult> {
            self.calls.lock().unwrap().push("list_roots".into());
            Ok(ListRootsResult {
                roots: vec![Root {
                    uri: "test://root".into(),
                    name: Some("Test Root".into()),
                    _meta: None,
                }],
                _meta: None,
                _extra: Default::default(),
            })
        }
    }

    struct TestServerHandler;

    #[derive(Clone)]
    struct NegotiatingServer {
        received: Arc<Mutex<Vec<ProtocolVersion>>>,
    }

    #[derive(Clone)]
    struct RejectingServer {
        initialized_notifications: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ServerHandler for TestServerHandler {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("test-server").with_version("1.0.0"))
        }

        async fn pong(&self, _ctx: &ServerCtx) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl ServerHandler for NegotiatingServer {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            self.received.lock().unwrap().push(protocol_version);
            Ok(InitializeResult::new("negotiating-server"))
        }
    }

    #[async_trait]
    impl ServerHandler for RejectingServer {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("rejecting-server"))
        }

        async fn notification(
            &self,
            _ctx: &ServerCtx,
            notification: ClientNotification,
        ) -> Result<()> {
            if matches!(notification, ClientNotification::Initialized { .. }) {
                self.initialized_notifications
                    .fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    fn versions(values: &[&str]) -> SupportedProtocolVersions {
        SupportedProtocolVersions::new(values.iter().map(|value| value.parse().unwrap())).unwrap()
    }

    #[tokio::test]
    async fn client_connection_trait_methods() {
        let connection = TestClientHandler::default();

        let (tx, _) = mpsc::channel(4);
        let ctx = test_client_ctx(tx);

        connection.pong(&ctx).await.expect("Ping failed");

        let params = CreateMessageParams::user_message("Hello").with_max_tokens(1000);

        let result = connection
            .create_message(&ctx, "test", params)
            .await
            .expect("Create message failed");
        assert_eq!(result.model, "test-model");

        let roots = connection.list_roots(&ctx).await.unwrap();
        assert_eq!(roots.roots.len(), 1);

        let calls = connection.calls.lock().unwrap();
        assert!(calls.contains(&"ping".to_string()));
        assert!(calls.contains(&"create_message".to_string()));
        assert!(calls.contains(&"list_roots".to_string()));
    }

    #[tokio::test]
    async fn client_server_ping() {
        fmt::try_init().ok();

        let calls = Arc::new(Mutex::new(Vec::new()));

        let (mut client, handle) = connected_client_and_server_with_conn(
            || TestServerHandler,
            TestClientHandler {
                calls: calls.clone(),
            },
        )
        .await
        .expect("setup");

        client.init().await.expect("client init");
        client.ping().await.expect("client ping");

        {
            let list = calls.lock().unwrap();
            assert!(list.contains(&"on_connect".to_string()));
        }

        shutdown_client_and_server(client, handle).await;
    }

    #[tokio::test]
    async fn server_falls_back_to_latest_configured_version() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let server_received = received.clone();
        let server = tmcp::Server::new(move || NegotiatingServer {
            received: server_received.clone(),
        })
        .with_protocol_versions(versions(&["2025-06-18"]));
        let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();
        let handle = tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .unwrap();
        let mut client = tmcp::Client::new("test-client", "1.0.0")
            .with_protocol_versions(versions(&["2025-03-26", "2025-06-18"]));
        client
            .connect_stream_raw(client_reader, client_writer)
            .await
            .unwrap();

        let result = client.init().await.unwrap();
        assert_eq!(result.protocol_version.as_str(), "2025-06-18");
        assert_eq!(received.lock().unwrap()[0].as_str(), "2025-06-18");
        shutdown_client_and_server(client, handle).await;
    }

    #[tokio::test]
    async fn client_rejects_unsupported_server_selection_before_on_connect() {
        let initialized_notifications = Arc::new(AtomicUsize::new(0));
        let server_notifications = initialized_notifications.clone();
        let server = tmcp::Server::new(move || RejectingServer {
            initialized_notifications: server_notifications.clone(),
        })
        .with_protocol_versions(versions(&["2025-06-18"]));
        let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();
        let handle = tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut client = tmcp::Client::new("test-client", "1.0.0")
            .with_handler(TestClientHandler {
                calls: calls.clone(),
            })
            .with_protocol_versions(versions(&["2025-03-26"]));
        client
            .connect_stream_raw(client_reader, client_writer)
            .await
            .unwrap();

        let error = client.init().await.unwrap_err();
        assert!(matches!(error, tmcp::Error::Protocol(_)));
        assert!(!calls.lock().unwrap().contains(&"on_connect".to_string()));
        assert!(matches!(
            client.ping().await,
            Err(tmcp::Error::TransportDisconnected)
        ));
        handle.stop().await.unwrap();
        assert_eq!(initialized_notifications.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn client_disconnects_after_invalid_server_version() {
        let (server_reader, mut server_writer, client_reader, client_writer) = make_duplex_pair();
        let peer = tokio::spawn(async move {
            let mut lines = BufReader::new(server_reader).lines();
            let request = lines.next_line().await.unwrap().unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": {
                    "protocolVersion": "not-a-version",
                    "capabilities": {},
                    "serverInfo": {"name": "invalid-server", "version": "1.0.0"}
                }
            });
            server_writer
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut client =
            tmcp::Client::new("test-client", "1.0.0").with_handler(TestClientHandler {
                calls: calls.clone(),
            });
        client
            .connect_stream_raw(client_reader, client_writer)
            .await
            .unwrap();

        let error = client.init().await.unwrap_err();
        assert!(matches!(error, tmcp::Error::Protocol(_)));
        assert!(!calls.lock().unwrap().contains(&"on_connect".to_string()));
        assert!(matches!(
            client.ping().await,
            Err(tmcp::Error::TransportDisconnected)
        ));
        peer.await.unwrap();
    }
}
