//! Client/server ping integration tests.

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tmcp::{
        ClientCtx, ClientHandler, Result, ServerAPI, ServerCtx, ServerHandler,
        schema::*,
        testutils::{
            connected_client_and_server_with_conn, shutdown_client_and_server, test_client_ctx,
        },
    };
    use tokio::{
        sync::broadcast,
        time::{Duration, sleep},
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
                role: Role::Assistant,
                content: SamplingContent::Text(TextContent {
                    text: "Test response".into(),
                    annotations: None,
                    _meta: None,
                }),
                model: "test-model".into(),
                stop_reason: None,
                _meta: None,
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
            })
        }
    }

    struct TestServerHandler;

    #[async_trait]
    impl ServerHandler for TestServerHandler {
        async fn initialize(
            &self,
            _ctx: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("test-server").with_version("1.0.0"))
        }

        async fn pong(&self, _ctx: &ServerCtx) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn client_connection_trait_methods() {
        let connection = TestClientHandler::default();

        let (tx, _) = broadcast::channel(10);
        let ctx = test_client_ctx(tx);

        connection.pong(&ctx).await.expect("Ping failed");

        let params = CreateMessageParams {
            messages: vec![SamplingMessage {
                role: Role::User,
                content: SamplingContent::Text(TextContent {
                    text: "Hello".into(),
                    annotations: None,
                    _meta: None,
                }),
            }],
            system_prompt: None,
            include_context: None,
            temperature: None,
            max_tokens: 1000,
            metadata: None,
            stop_sequences: None,
            model_preferences: None,
        };

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
            || Box::new(TestServerHandler),
            TestClientHandler {
                calls: calls.clone(),
            },
        )
        .await
        .expect("setup");

        sleep(Duration::from_millis(10)).await;

        {
            let list = calls.lock().unwrap();
            assert!(list.contains(&"on_connect".to_string()));
        }

        calls.lock().unwrap().clear();

        client.ping().await.expect("client ping");

        shutdown_client_and_server(client, handle).await;
    }
}
