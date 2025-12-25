//! Lifecycle behavior tests for clients and servers.

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use async_trait::async_trait;
    use tmcp::{Client, Result, Server, ServerCtx, ServerHandler, schema::*, testutils::*};
    use tokio::{
        net::TcpListener,
        sync::Mutex,
        time::{Duration, sleep},
    };

    /// Test server that tracks lifecycle events
    #[derive(Debug, Clone)]
    struct LifecycleTestServer {
        connect_count: Arc<AtomicU32>,
        shutdown_count: Arc<AtomicU32>,
        connect_addrs: Arc<Mutex<Vec<String>>>,
    }

    impl Default for LifecycleTestServer {
        fn default() -> Self {
            Self {
                connect_count: Arc::new(AtomicU32::new(0)),
                shutdown_count: Arc::new(AtomicU32::new(0)),
                connect_addrs: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ServerHandler for LifecycleTestServer {
        async fn on_connect(&self, _ctx: &ServerCtx, remote_addr: &str) -> Result<()> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let mut addrs = self.connect_addrs.lock().await;
            addrs.push(remote_addr.to_string());
            Ok(())
        }

        async fn on_shutdown(&self) -> Result<()> {
            self.shutdown_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("lifecycle_test_server").with_version("1.0.0"))
        }
    }

    #[tokio::test]
    async fn test_stdio_lifecycle() {
        // Test using stream transport (simulates stdio in tests)
        let server_impl = LifecycleTestServer::default();
        let server_clone = server_impl.clone();
        let server = Server::new(move || server_clone.clone());

        // Create stream pair
        let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();
        let server_handle = tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .unwrap();

        // Connect client
        let mut client = Client::new("test-client", "1.0.0");
        client
            .connect_stream(client_reader, client_writer)
            .await
            .unwrap();
        client.list_tools(None).await.ok();

        // Verify on_connect was called
        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 1);
        let addrs = server_impl.connect_addrs.lock().await;
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], "unknown"); // StreamTransport reports "unknown"
        drop(addrs);

        // Disconnect
        drop(client);
        server_handle.stop().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        // Verify on_shutdown was called
        assert_eq!(server_impl.shutdown_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_tcp_lifecycle() {
        // Test TCP transport with manual accept loop
        let server_impl = Arc::new(LifecycleTestServer::default());
        let server_impl_for_factory = server_impl.clone();

        // Start TCP listener
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn accept loop
        let server_task = tokio::spawn(async move {
            while let Ok((stream, _peer_addr)) = listener.accept().await {
                let server_impl_clone = server_impl_for_factory.clone();
                let server = Server::new(move || (*server_impl_clone).clone());

                tokio::spawn(async move {
                    let (read, write) = stream.into_split();
                    server.serve_stream(read, write).await.ok();
                });
            }
        });

        // Connect client
        let mut client = Client::new("tcp-client", "1.0.0");
        client.connect_tcp(&addr.to_string()).await.unwrap();

        // Verify on_connect
        sleep(Duration::from_millis(100)).await;
        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 1);

        // Disconnect
        drop(client);
        sleep(Duration::from_millis(200)).await;

        // Clean up
        server_task.abort();
        server_task.await.ok();
    }

    #[tokio::test]
    async fn test_http_lifecycle() {
        // Test HTTP transport
        let server_impl = LifecycleTestServer::default();
        let server_clone = server_impl.clone();
        let server = Server::new(move || server_clone.clone());

        // Start HTTP server
        let http_server = server.serve_http("127.0.0.1:0").await.unwrap();
        let addr = http_server.bound_addr.clone().unwrap();

        // Connect first client
        let mut client1 = Client::new("http-client-1", "1.0.0");
        client1
            .connect_http(&format!("http://{addr}"))
            .await
            .unwrap();
        client1.list_tools(None).await.ok();

        // HTTP server has single lifecycle - one on_connect for all clients
        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 1);
        let addrs = server_impl.connect_addrs.lock().await;
        assert_eq!(addrs[0], addr);
        drop(addrs);

        // Connect second client - should not trigger another on_connect
        let mut client2 = Client::new("http-client-2", "1.0.0");
        client2
            .connect_http(&format!("http://{addr}"))
            .await
            .unwrap();
        client2.list_tools(None).await.ok();

        // Still only one on_connect
        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 1);

        // Make requests to verify connections work
        client1.list_tools(None).await.ok();
        client2.list_tools(None).await.ok();

        // Clean up
        drop(client1);
        drop(client2);

        // Stop the HTTP server
        http_server.stop().await.unwrap();
        sleep(Duration::from_millis(200)).await;

        // Verify on_shutdown - HTTP server calls on_shutdown when the server stops
        assert_eq!(server_impl.shutdown_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_connections() {
        // Test multiple connections to verify each gets its own lifecycle
        let server_impl = LifecycleTestServer::default();

        // First connection
        let server_clone = server_impl.clone();
        let (mut client1, handle1) = connected_client_and_server(move || {
            Box::new(server_clone.clone()) as Box<dyn ServerHandler>
        })
        .await
        .unwrap();
        client1.init().await.unwrap();
        client1.list_tools(None).await.ok();

        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 1);

        // Second connection
        let server_clone2 = server_impl.clone();
        let (mut client2, handle2) = connected_client_and_server(move || {
            Box::new(server_clone2.clone()) as Box<dyn ServerHandler>
        })
        .await
        .unwrap();
        client2.init().await.unwrap();
        client2.list_tools(None).await.ok();

        assert_eq!(server_impl.connect_count.load(Ordering::SeqCst), 2);

        // Disconnect first client
        drop(client1);
        handle1.stop().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        assert_eq!(server_impl.shutdown_count.load(Ordering::SeqCst), 1);

        // Disconnect second client
        drop(client2);
        handle2.stop().await.unwrap();
        sleep(Duration::from_millis(100)).await;

        assert_eq!(server_impl.shutdown_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_client_server_interaction() {
        // Test basic client-server interaction
        let server = Server::new(LifecycleTestServer::default);
        let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();
        let _server_handle = tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
            .await
            .unwrap();

        // Create client
        let mut client = Client::new("test-client", "1.0.0");
        client
            .connect_stream(client_reader, client_writer)
            .await
            .unwrap();

        // Verify client can make requests
        let tools = client.list_tools(None).await.unwrap();
        assert!(tools.tools.is_empty()); // LifecycleTestServer doesn't implement tools

        // Clean up
        drop(client);
    }

    #[tokio::test]
    async fn test_remote_addr_reporting() {
        // Test that different transports report correct remote addresses
        let server_impl = LifecycleTestServer::default();

        // Test 1: Stream transport reports "unknown"
        {
            let server_clone = server_impl.clone();
            let server = Server::new(move || server_clone.clone());
            let (server_reader, server_writer, client_reader, client_writer) = make_duplex_pair();
            let server_handle =
                tmcp::ServerHandle::from_stream(server, server_reader, server_writer)
                    .await
                    .unwrap();

            let mut client = Client::new("stream-client", "1.0.0");
            client
                .connect_stream(client_reader, client_writer)
                .await
                .unwrap();
            client.list_tools(None).await.ok();

            let addrs = server_impl.connect_addrs.lock().await;
            assert_eq!(addrs.last().unwrap(), "unknown");
            drop(addrs);

            drop(client);
            server_handle.stop().await.unwrap();
        }

        // Test 2: HTTP reports server address
        {
            let server_clone = server_impl.clone();
            let server = Server::new(move || server_clone.clone());
            let http_server = server.serve_http("127.0.0.1:0").await.unwrap();
            let addr = http_server.bound_addr.clone().unwrap();

            let mut client = Client::new("http-client", "1.0.0");
            client
                .connect_http(&format!("http://{addr}"))
                .await
                .unwrap();
            client.list_tools(None).await.ok();

            let addrs = server_impl.connect_addrs.lock().await;
            assert_eq!(addrs.last().unwrap(), &addr);
            drop(addrs);

            drop(client);
            http_server.stop().await.unwrap();
        }
    }
}
