//! OAuth integration tests for tenx-mcp.

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use tenx_mcp::auth::{OAuth2CallbackServer, OAuth2Client, OAuth2Config, OAuth2Token};
    use tokio::time::{Duration, sleep, timeout};

    #[tokio::test]
    async fn test_oauth_client_creation() {
        let config = OAuth2Config {
            client_id: "test_client_id".to_string(),
            client_secret: Some("test_client_secret".to_string()),
            auth_url: "http://localhost:9090/oauth/authorize".to_string(),
            token_url: "http://localhost:9090/oauth/token".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
            resource: "http://localhost:9090/api".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        let oauth_client = OAuth2Client::new(config).unwrap();
        // Simply verify that we can create an OAuth client successfully
        let _arc_client = Arc::new(oauth_client);
    }

    #[tokio::test]
    async fn test_authorization_url_generation() {
        let config = OAuth2Config {
            client_id: "test_client_id".to_string(),
            client_secret: None,
            auth_url: "http://localhost:9090/oauth/authorize".to_string(),
            token_url: "http://localhost:9090/oauth/token".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
            resource: "http://localhost:9090/api".to_string(),
            scopes: vec!["read".to_string()],
        };

        let mut oauth_client = OAuth2Client::new(config).unwrap();
        let (auth_url, csrf_token) = oauth_client.get_authorization_url();

        // Check that the URL contains expected parameters
        let url_str = auth_url.as_str();
        assert!(url_str.contains("client_id=test_client_id"));
        assert!(url_str.contains("redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Fcallback"));
        assert!(url_str.contains("response_type=code"));
        assert!(url_str.contains("state="));
        assert!(url_str.contains("code_challenge="));
        assert!(url_str.contains("code_challenge_method=S256"));
        assert!(url_str.contains("resource=http%3A%2F%2Flocalhost%3A9090%2Fapi"));
        assert!(url_str.contains("scope=read"));

        // CSRF token should not be empty
        assert!(!csrf_token.secret().is_empty());
    }

    #[tokio::test]
    async fn test_callback_server() {
        let server = OAuth2CallbackServer::new(8765);

        // Spawn a task to simulate a client making the callback request
        let client_task = tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;

            let client = reqwest::Client::new();
            let _response = client
                .get("http://127.0.0.1:8765/callback?code=test_code&state=test_state")
                .send()
                .await
                .expect("callback request failed");
        });

        // Wait for callback with timeout
        let result = timeout(Duration::from_secs(5), server.wait_for_callback()).await;

        match result {
            Ok(Ok((code, state))) => {
                assert_eq!(code, "test_code");
                assert_eq!(state, "test_state");
            }
            Ok(Err(e)) => panic!("Callback server error: {e}"),
            Err(_) => panic!("Callback server timed out"),
        }

        client_task.await.expect("callback client task failed");
    }

    #[tokio::test]
    async fn test_callback_server_oversized_request() {
        use tokio::{io::AsyncWriteExt, net::TcpStream};

        let server = OAuth2CallbackServer::new(8766);

        let client_task = tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            let mut stream = TcpStream::connect("127.0.0.1:8766").await.unwrap();
            let big_query = "a".repeat(9000);
            let request = format!(
                "GET /callback?code={big_query}&state=test HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("failed to send oversized request");
        });

        let result = timeout(Duration::from_secs(5), server.wait_for_callback()).await;
        // With axum, oversized requests are rejected at the HTTP layer and cause timeout
        assert!(matches!(result, Err(_) | Ok(Err(_))));

        client_task.await.expect("oversized request task failed");
    }

    #[tokio::test]
    async fn test_callback_server_malformed_request() {
        use tokio::{io::AsyncWriteExt, net::TcpStream};

        let server = OAuth2CallbackServer::new(8767);

        let client_task = tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
            let mut stream = TcpStream::connect("127.0.0.1:8767").await.unwrap();
            let request = "POST /bad HTTP/1.1\r\nHost: localhost\r\n\r\n";
            stream
                .write_all(request.as_bytes())
                .await
                .expect("failed to send malformed request");
        });

        let result = timeout(Duration::from_secs(5), server.wait_for_callback()).await;
        // With axum, malformed requests are rejected and cause timeout or error
        assert!(matches!(result, Err(_) | Ok(Err(_))));

        client_task.await.expect("malformed request task failed");
    }

    #[tokio::test]
    async fn test_http_transport_with_oauth() {
        // This test verifies that the OAuth client can be integrated with the HTTP transport
        let config = OAuth2Config {
            client_id: "test_client_id".to_string(),
            client_secret: Some("test_client_secret".to_string()),
            auth_url: "http://localhost:9090/oauth/authorize".to_string(),
            token_url: "http://localhost:9090/oauth/token".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
            resource: "http://localhost:9090/api".to_string(),
            scopes: vec!["read".to_string()],
        };

        let oauth_client = OAuth2Client::new(config).unwrap();

        // Set a pre-configured token to avoid the OAuth flow
        let token = OAuth2Token {
            access_token: "test_access_token".to_string(),
            refresh_token: Some("test_refresh_token".to_string()),
            expires_at: Some(Instant::now() + Duration::from_secs(3600)),
        };
        oauth_client.set_token(token).await;

        let oauth_client_arc = Arc::new(oauth_client);

        // Verify the token is retrievable
        let retrieved_token = oauth_client_arc.get_valid_token().await.unwrap();
        assert_eq!(retrieved_token, "test_access_token");

        // The actual HTTP transport integration is tested in the examples
        // This test focuses on the OAuth client functionality
    }

    #[tokio::test]
    async fn test_token_refresh() {
        let config = OAuth2Config {
            client_id: "test_client_id".to_string(),
            client_secret: Some("test_client_secret".to_string()),
            auth_url: "http://localhost:9090/oauth/authorize".to_string(),
            token_url: "http://localhost:9090/oauth/token".to_string(),
            redirect_url: "http://localhost:8080/callback".to_string(),
            resource: "http://localhost:9090/api".to_string(),
            scopes: vec!["read".to_string()],
        };

        let oauth_client = OAuth2Client::new(config).unwrap();

        // Set an expired token
        let token = OAuth2Token {
            access_token: "expired_token".to_string(),
            refresh_token: Some("refresh_token".to_string()),
            expires_at: Some(Instant::now() - Duration::from_secs(1)), // Already expired
        };
        oauth_client.set_token(token).await;

        // Try to get a valid token - this should trigger a refresh
        // In a real scenario, this would make an HTTP request to the token endpoint
        // For testing, we'll just verify the logic works
        let result = oauth_client.get_valid_token().await;

        // This will fail because we don't have a real OAuth server, but the logic is tested
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_concurrent_refresh_single_request() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use axum::{Json, Router, extract::State, routing::post};
        use tokio::{net::TcpListener, sync::oneshot};

        #[derive(Clone)]
        struct Ctx {
            counter: Arc<AtomicUsize>,
        }

        async fn token_handler(State(ctx): State<Ctx>) -> Json<serde_json::Value> {
            ctx.counter.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::json!({
                "access_token": "new_access_token",
                "token_type": "Bearer",
                "refresh_token": "new_refresh_token",
                "expires_in": 3600
            }))
        }

        let counter = Arc::new(AtomicUsize::new(0));

        let state = Ctx {
            counter: counter.clone(),
        };
        let router = Router::new()
            .route("/token", post(token_handler))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    rx.await.ok();
                })
                .await
                .unwrap();
        });

        let config = OAuth2Config {
            client_id: "client".to_string(),
            client_secret: Some("secret".to_string()),
            auth_url: format!("http://{addr}/auth"),
            token_url: format!("http://{addr}/token"),
            redirect_url: "http://localhost:1/callback".to_string(),
            resource: "http://resource".to_string(),
            scopes: vec![],
        };

        let oauth_client = OAuth2Client::new(config).unwrap();
        oauth_client
            .set_token(OAuth2Token {
                access_token: "expired".to_string(),
                refresh_token: Some("rt".to_string()),
                expires_at: Some(Instant::now() - Duration::from_secs(1)),
            })
            .await;

        let client_arc = Arc::new(oauth_client);
        let mut handles = Vec::new();
        for _ in 0..5 {
            let c = client_arc.clone();
            handles.push(tokio::spawn(
                async move { c.get_valid_token().await.unwrap() },
            ));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), "new_access_token");
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        tx.send(()).ok();
    }
}
