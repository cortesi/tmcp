//! OAuth integration tests for tmcp.

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex as StdMutex},
        time::Instant,
    };

    use async_trait::async_trait;
    use tmcp::auth::{
        ClientMetadata, DynamicRegistrationConfig, OAuth2CallbackServer, OAuth2Client,
        OAuth2Config, OAuth2Token, TokenStore, TokenStoreGuard,
    };
    use tokio::{
        net::TcpStream,
        sync::Mutex as TokioMutex,
        time::{Duration, timeout},
    };

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

        let oauth_client = OAuth2Client::new(config).unwrap();
        let flow = oauth_client.begin_authorization();

        // Check that the URL contains expected parameters
        let url_str = flow.auth_url().as_str();
        assert!(url_str.contains("client_id=test_client_id"));
        assert!(url_str.contains("redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Fcallback"));
        assert!(url_str.contains("response_type=code"));
        assert!(url_str.contains("state="));
        assert!(url_str.contains("code_challenge="));
        assert!(url_str.contains("code_challenge_method=S256"));
        assert!(url_str.contains("resource=http%3A%2F%2Flocalhost%3A9090%2Fapi"));
        assert!(url_str.contains("scope=read"));
    }

    #[tokio::test]
    async fn test_callback_server() {
        let server = OAuth2CallbackServer::bind_loopback()
            .await
            .expect("callback server");
        assert_ne!(server.port(), 0);
        let redirect_url = server.redirect_url();

        // The listener is bound eagerly, so the client can connect immediately.
        let client_task = tokio::spawn(async move {
            let client = reqwest::Client::new();
            let _response = client
                .get(format!("{redirect_url}?code=test_code&state=test_state"))
                .send()
                .await
                .expect("callback request failed");
        });

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
    async fn test_callback_server_ignores_other_paths() {
        let server = OAuth2CallbackServer::bind_loopback()
            .await
            .expect("callback server");
        let base_url = format!("http://127.0.0.1:{}", server.port());

        let client_task = tokio::spawn(async move {
            let client = reqwest::Client::new();

            // A stray request (e.g. a favicon probe) receives 404 and the
            // server keeps waiting for the real callback.
            let favicon = client
                .get(format!("{base_url}/favicon.ico"))
                .send()
                .await
                .expect("favicon request failed");
            assert_eq!(favicon.status(), reqwest::StatusCode::NOT_FOUND);

            let callback = client
                .get(format!(
                    "{base_url}/callback?code=test_code&state=test_state"
                ))
                .send()
                .await
                .expect("callback request failed");
            assert_eq!(callback.status(), reqwest::StatusCode::OK);
        });

        let (code, state) = timeout(Duration::from_secs(5), server.wait_for_callback())
            .await
            .expect("callback server timed out")
            .expect("callback failed");
        assert_eq!(code, "test_code");
        assert_eq!(state, "test_state");

        client_task.await.expect("callback client task failed");
    }

    #[tokio::test]
    async fn test_callback_server_surfaces_oauth_errors() {
        let server = OAuth2CallbackServer::bind_loopback()
            .await
            .expect("callback server");
        let redirect_url = server.redirect_url();

        let client_task = tokio::spawn(async move {
            let client = reqwest::Client::new();
            let response = client
                .get(format!(
                    "{redirect_url}?error=access_denied&error_description=User%20cancelled"
                ))
                .send()
                .await
                .expect("error redirect failed");
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        });

        let err = timeout(Duration::from_secs(5), server.wait_for_callback())
            .await
            .expect("callback server timed out")
            .expect_err("expected OAuth error");
        let message = err.to_string();
        assert!(message.contains("access_denied"), "got: {message}");
        assert!(message.contains("User cancelled"), "got: {message}");

        client_task.await.expect("error redirect task failed");
    }

    #[tokio::test]
    async fn test_callback_server_oversized_request() {
        use tokio::io::AsyncWriteExt;

        let server = OAuth2CallbackServer::bind_loopback()
            .await
            .expect("callback server");
        let addr = format!("127.0.0.1:{}", server.port());

        let client_task = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.expect("connect");
            let big_query = "a".repeat(3000);
            let request = format!(
                "GET /callback?code={big_query}&state=test HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("failed to send oversized request");
        });

        let result = server.wait_for_callback().await;
        assert!(result.is_err());

        client_task.await.expect("oversized request task failed");
    }

    #[tokio::test]
    async fn test_callback_server_malformed_request() {
        use tokio::io::AsyncWriteExt;

        let server = OAuth2CallbackServer::bind_loopback()
            .await
            .expect("callback server");
        let addr = format!("127.0.0.1:{}", server.port());

        let client_task = tokio::spawn(async move {
            let mut stream = TcpStream::connect(&addr).await.expect("connect");
            let request = "GET /callback?code=test_code HTTP/1.1\r\nHost: localhost\r\n\r\n";
            stream
                .write_all(request.as_bytes())
                .await
                .expect("failed to send malformed request");
        });

        let result = server.wait_for_callback().await;
        assert!(result.is_err());

        client_task.await.expect("malformed request task failed");
    }

    #[tokio::test]
    async fn test_http_transport_with_oauth() {
        // This test verifies that the OAuth client can be integrated with the
        // HTTP transport
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
            expires_in: Some(Duration::from_secs(3600)),
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
    async fn test_register_dynamic_with_caller_metadata() {
        use axum::{Json, Router, extract::State, routing::post};
        use serde_json::Value;
        use tokio::{net::TcpListener, sync::oneshot};

        #[derive(Clone)]
        struct Ctx {
            captured: Arc<StdMutex<Option<Value>>>,
        }

        async fn registration_handler(
            State(ctx): State<Ctx>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            *ctx.captured.lock().expect("captured registration body") = Some(body.clone());

            let mut response = body.as_object().cloned().unwrap_or_default();
            response.insert(
                "client_id".to_string(),
                Value::String("registered".to_string()),
            );
            Json(Value::Object(response))
        }

        let captured = Arc::new(StdMutex::new(None));
        let router = Router::new()
            .route("/register", post(registration_handler))
            .with_state(Ctx {
                captured: captured.clone(),
            });

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

        let mut metadata = ClientMetadata::new("Custom", "http://localhost/callback")
            .with_resource("http://localhost/resource")
            .with_token_endpoint_auth_method("none");
        metadata.additional.insert(
            "software_statement".to_string(),
            Value::String("signed-statement".to_string()),
        );

        let oauth_client = OAuth2Client::register_dynamic(DynamicRegistrationConfig {
            auth_url: format!("http://{addr}/authorize"),
            token_url: format!("http://{addr}/token"),
            resource: "http://localhost/resource".to_string(),
            registration_endpoint: Some(format!("http://{addr}/register")),
            metadata,
        })
        .await
        .unwrap();

        let registered = captured
            .lock()
            .expect("captured registration body")
            .take()
            .unwrap();
        assert_eq!(registered["client_name"], "Custom");
        assert_eq!(registered["software_statement"], "signed-statement");
        assert_eq!(registered["token_endpoint_auth_method"], "none");

        drop(oauth_client);
        tx.send(()).ok();
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
            expires_in: Some(Duration::from_secs(3600)),
            expires_at: Some(Instant::now() - Duration::from_secs(1)), // Already expired
        };
        oauth_client.set_token(token).await;
        let revisions = oauth_client.subscribe_token_revisions();
        let revision = *revisions.borrow();

        // Try to get a valid token - this should trigger a refresh
        // In a real scenario, this would make an HTTP request to the token
        // endpoint For testing, we'll just verify the logic works
        let result = oauth_client.get_valid_token().await;

        // This will fail because we don't have a real OAuth server, but the
        // logic is tested
        assert!(result.is_err());
        assert_eq!(*revisions.borrow(), revision);
        assert!(!revisions.has_changed().unwrap());
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
                expires_in: Some(Duration::from_secs(3600)),
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

    #[tokio::test]
    async fn test_reactive_refresh_skips_when_another_task_refreshed() {
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
                "access_token": "refreshed_access_token",
                "token_type": "Bearer",
                "refresh_token": "new_refresh_token",
                "expires_in": 3600
            }))
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/token",
            post(token_handler).with_state(Ctx {
                counter: counter.clone(),
            }),
        );

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
                access_token: "new_access_token".to_string(),
                refresh_token: Some("rt".to_string()),
                expires_in: Some(Duration::from_secs(3600)),
                expires_at: Some(Instant::now() + Duration::from_secs(3600)),
            })
            .await;
        let mut revisions = oauth_client.subscribe_token_revisions();
        let initial_revision = *revisions.borrow_and_update();

        let token = oauth_client
            .refresh_access_token_if_current("old_access_token")
            .await
            .unwrap();

        assert_eq!(token, "new_access_token");
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(!revisions.has_changed().unwrap());

        let token = oauth_client
            .refresh_access_token_if_current("new_access_token")
            .await
            .unwrap();

        assert_eq!(token, "refreshed_access_token");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        revisions.changed().await.unwrap();
        assert_eq!(*revisions.borrow_and_update(), initial_revision + 1);
        assert_eq!(
            oauth_client
                .current_token()
                .await
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("new_refresh_token")
        );

        let refreshed = oauth_client.refresh_access_token().await.unwrap();
        assert_eq!(refreshed.access_token, "refreshed_access_token");
        assert_eq!(
            refreshed.refresh_token.as_deref(),
            Some("new_refresh_token")
        );
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        revisions.changed().await.unwrap();
        assert_eq!(*revisions.borrow_and_update(), initial_revision + 2);

        tx.send(()).ok();
    }

    /// In-memory `TokenStore` test double. Lets tests observe every `save`
    /// call and simulate a deleted credential with an empty store.
    #[derive(Clone)]
    struct MemoryTokenStore {
        state: Arc<StdMutex<Option<OAuth2Token>>>,
        excl: Arc<TokioMutex<()>>,
        saves: Arc<StdMutex<Vec<(OAuth2Token, OAuth2Token)>>>,
    }

    impl MemoryTokenStore {
        fn new(token: Option<OAuth2Token>) -> Self {
            Self {
                state: Arc::new(StdMutex::new(token)),
                excl: Arc::new(TokioMutex::new(())),
                saves: Arc::new(StdMutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl TokenStore for MemoryTokenStore {
        async fn lock(&self) -> Result<TokenStoreGuard, tmcp::Error> {
            let guard = self.excl.clone().lock_owned().await;
            Ok(Box::new(guard))
        }

        async fn load(&self) -> Result<Option<OAuth2Token>, tmcp::Error> {
            Ok(self.state.lock().unwrap().clone())
        }

        async fn save(
            &self,
            previous: &OAuth2Token,
            token: &OAuth2Token,
        ) -> Result<(), tmcp::Error> {
            self.saves
                .lock()
                .unwrap()
                .push((previous.clone(), token.clone()));
            *self.state.lock().unwrap() = Some(token.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_get_valid_token_returns_fresh_stored_token_without_request() {
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
                "access_token": "should_not_be_requested",
                "token_type": "Bearer",
                "expires_in": 3600
            }))
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/token",
            post(token_handler).with_state(Ctx {
                counter: counter.clone(),
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let server = tokio::spawn(async move {
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

        let store_token = OAuth2Token {
            access_token: "fresh_from_store".to_string(),
            refresh_token: Some("store_refresh".to_string()),
            expires_in: Some(Duration::from_secs(3600)),
            expires_at: Some(Instant::now() + Duration::from_secs(3600)),
        };
        let store = MemoryTokenStore::new(Some(store_token));

        let oauth_client = OAuth2Client::new(config)
            .unwrap()
            .with_token_store(Arc::new(store));
        oauth_client
            .set_token(OAuth2Token {
                access_token: "expired_local".to_string(),
                refresh_token: Some("local_refresh".to_string()),
                expires_in: Some(Duration::from_secs(3600)),
                expires_at: Some(Instant::now() - Duration::from_secs(1)),
            })
            .await;

        let token = oauth_client.get_valid_token().await.unwrap();
        assert_eq!(token, "fresh_from_store");
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(
            oauth_client.current_token().await.unwrap().access_token,
            "fresh_from_store"
        );

        tx.send(()).ok();
        server.abort();
    }

    #[tokio::test]
    async fn test_refresh_saves_new_token_with_previous() {
        use axum::{Json, Router, routing::post};
        use tokio::{net::TcpListener, sync::oneshot};

        async fn token_handler() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "access_token": "new_access",
                "token_type": "Bearer",
                "refresh_token": "new_refresh",
                "expires_in": 3600
            }))
        }

        let router = Router::new().route("/token", post(token_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let server = tokio::spawn(async move {
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

        let initial = OAuth2Token {
            access_token: "initial_access".to_string(),
            refresh_token: Some("initial_refresh".to_string()),
            expires_in: Some(Duration::from_secs(3600)),
            expires_at: Some(Instant::now() - Duration::from_secs(1)),
        };
        let store = MemoryTokenStore::new(Some(initial.clone()));

        let oauth_client = OAuth2Client::new(config)
            .unwrap()
            .with_token_store(Arc::new(store.clone()));
        oauth_client.set_token(initial).await;

        let refreshed = oauth_client.refresh_access_token().await.unwrap();
        assert_eq!(refreshed.access_token, "new_access");

        let saves = store.saves.lock().unwrap().clone();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].0.access_token, "initial_access");
        assert_eq!(saves[0].1.access_token, "new_access");

        let stored = store.load().await.unwrap().unwrap();
        assert_eq!(stored.access_token, "new_access");

        tx.send(()).ok();
        server.abort();
    }

    #[tokio::test]
    async fn test_missing_stored_credential_fails_refresh_without_request_and_clears_cache() {
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
                "access_token": "unused",
                "token_type": "Bearer",
                "expires_in": 3600
            }))
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/token",
            post(token_handler).with_state(Ctx {
                counter: counter.clone(),
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let server = tokio::spawn(async move {
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

        let store = MemoryTokenStore::new(None);
        let oauth_client = OAuth2Client::new(config)
            .unwrap()
            .with_token_store(Arc::new(store));
        oauth_client
            .set_token(OAuth2Token {
                access_token: "cached".to_string(),
                refresh_token: Some("cached_refresh".to_string()),
                expires_in: Some(Duration::from_secs(3600)),
                expires_at: Some(Instant::now() - Duration::from_secs(1)),
            })
            .await;

        let err = oauth_client.get_valid_token().await.unwrap_err();
        assert!(err.to_string().contains("no longer exists"), "got: {err}");
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(oauth_client.current_token().await.is_none());

        tx.send(()).ok();
        server.abort();
    }

    #[tokio::test]
    async fn test_refresh_access_token_if_current_returns_stored_token_without_request() {
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
                "access_token": "unused",
                "token_type": "Bearer",
                "expires_in": 3600
            }))
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/token",
            post(token_handler).with_state(Ctx {
                counter: counter.clone(),
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let server = tokio::spawn(async move {
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

        let store_token = OAuth2Token {
            access_token: "rotated_by_other_holder".to_string(),
            refresh_token: Some("other_refresh".to_string()),
            expires_in: Some(Duration::from_secs(3600)),
            expires_at: Some(Instant::now() + Duration::from_secs(3600)),
        };
        let store = MemoryTokenStore::new(Some(store_token));

        let oauth_client = OAuth2Client::new(config)
            .unwrap()
            .with_token_store(Arc::new(store));
        oauth_client
            .set_token(OAuth2Token {
                access_token: "sent_token".to_string(),
                refresh_token: Some("sent_refresh".to_string()),
                expires_in: Some(Duration::from_secs(3600)),
                expires_at: Some(Instant::now() + Duration::from_secs(3600)),
            })
            .await;

        let token = oauth_client
            .refresh_access_token_if_current("sent_token")
            .await
            .unwrap();

        assert_eq!(token, "rotated_by_other_holder");
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        tx.send(()).ok();
        server.abort();
    }

    #[tokio::test]
    async fn test_refresh_times_out_near_configured_duration() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hang_server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                // Accept and hold connections open without ever responding,
                // so the client hangs waiting for a response.
                held.push(stream);
            }
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

        let oauth_client = OAuth2Client::new(config)
            .unwrap()
            .with_refresh_timeout(Duration::from_millis(200));
        oauth_client
            .set_token(OAuth2Token {
                access_token: "expired".to_string(),
                refresh_token: Some("rt".to_string()),
                expires_in: Some(Duration::from_secs(3600)),
                expires_at: Some(Instant::now() - Duration::from_secs(1)),
            })
            .await;

        let started = Instant::now();
        let result = timeout(Duration::from_secs(5), oauth_client.get_valid_token()).await;
        let elapsed = started.elapsed();

        let err = result
            .expect("refresh did not honor its own timeout")
            .expect_err("expected the refresh to fail");
        assert!(
            err.to_string().to_lowercase().contains("timed out"),
            "got: {err}"
        );
        assert!(
            elapsed >= Duration::from_millis(100),
            "refresh returned too quickly: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "refresh took too long: {elapsed:?}"
        );

        hang_server.abort();
    }
}
