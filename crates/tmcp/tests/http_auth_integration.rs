//! HTTP auth integration tests.

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use async_trait::async_trait;
    use jsonwebtoken::{
        Algorithm, EncodingKey, Header, encode,
        jwk::{Jwk, JwkSet, KeyAlgorithm},
    };
    use reqwest::Client as HttpClient;
    use rsa::{
        RsaPrivateKey,
        pkcs8::{EncodePrivateKey, LineEnding},
        rand_core::OsRng,
    };
    use serde_json::{Value, json};
    use tmcp::{
        Arguments, Result, Server, ServerCtx, ServerHandler,
        auth::{
            ProtectedResourceMetadata,
            server::{AuthConfig, AuthInfo, JwtValidator},
        },
        schema::*,
    };
    use tracing_subscriber::fmt;

    struct AuthenticatedConnection;

    #[async_trait]
    impl ServerHandler for AuthenticatedConnection {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("oauth-server")
                .with_version("0.1.0")
                .with_capabilities(ServerCapabilities::default().with_tools(None)))
        }

        async fn list_tools(
            &self,
            _context: &ServerCtx,
            _cursor: Option<Cursor>,
        ) -> Result<ListToolsResult> {
            Ok(ListToolsResult::default().with_tool(
                Tool::new("whoami", ToolSchema::default()).with_description("Return auth info"),
            ))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResult> {
            if name != "whoami" {
                return Err(tmcp::Error::ToolNotFound(name));
            }

            let auth = context.extensions().get::<AuthInfo>().unwrap();
            let mut scopes = auth.scopes.iter().cloned().collect::<Vec<_>>();
            scopes.sort();

            Ok(CallToolResult::new().with_text_content(format!(
                "subject={};audiences={};scopes={}",
                auth.subject,
                auth.audiences.join(","),
                scopes.join(","),
            )))
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn signing_key(kid: &str) -> (EncodingKey, JwkSet) {
        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
        let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some(kid.to_string());
        jwk.common.key_algorithm = Some(KeyAlgorithm::RS256);
        (encoding_key, JwkSet { keys: vec![jwk] })
    }

    fn token(key: &EncodingKey, kid: &str, exp: u64) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            &json!({
                "sub": "user-123",
                "iss": "https://issuer.example.com",
                "aud": ["tmcp"],
                "exp": exp,
                "scope": "resources:read tools:call",
            }),
            key,
        )
        .unwrap()
    }

    fn initialize_payload() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": LATEST_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "auth-test-client",
                    "version": "0.1.0"
                }
            }
        })
    }

    fn call_tool_payload() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "whoami"
            }
        })
    }

    fn protected_resource_metadata() -> ProtectedResourceMetadata {
        ProtectedResourceMetadata {
            resource: "https://example.com/mcp".to_string(),
            authorization_servers: vec!["https://issuer.example.com".to_string()],
            scopes_supported: Some(vec!["resources:read".to_string(), "tools:call".to_string()]),
            bearer_methods_supported: Some(vec!["header".to_string()]),
            resource_documentation: None,
            additional: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_http_auth_flow() {
        fmt::try_init().ok();

        let (encoding_key, jwk_set) = signing_key("kid-1");
        let validator = Arc::new(JwtValidator::from_jwk_set(
            "https://issuer.example.com",
            ["tmcp"],
            jwk_set,
        ));
        let server_handle = Server::new(|| AuthenticatedConnection)
            .http("127.0.0.1:0")
            .with_auth(AuthConfig::new(protected_resource_metadata(), validator))
            .serve()
            .await
            .unwrap();

        let base_url = format!("http://{}", server_handle.bound_addr.as_ref().unwrap());
        let client = HttpClient::new();

        let no_auth = client
            .post(format!("{base_url}/"))
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .json(&initialize_payload())
            .send()
            .await
            .unwrap();
        assert_eq!(no_auth.status(), reqwest::StatusCode::UNAUTHORIZED);
        let challenge = no_auth.headers()["www-authenticate"].to_str().unwrap();
        assert_eq!(
            challenge,
            "Bearer resource_metadata=\"/.well-known/oauth-protected-resource\""
        );
        assert!(!challenge.contains("error="));

        let valid_token = token(&encoding_key, "kid-1", now() + 300);
        let init = client
            .post(format!("{base_url}/"))
            .bearer_auth(&valid_token)
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .json(&initialize_payload())
            .send()
            .await
            .unwrap();
        assert_eq!(init.status(), reqwest::StatusCode::OK);
        let session_id = init.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();

        let call = client
            .post(format!("{base_url}/"))
            .bearer_auth(&valid_token)
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .header("Mcp-Session-Id", &session_id)
            .json(&call_tool_payload())
            .send()
            .await
            .unwrap();
        assert_eq!(call.status(), reqwest::StatusCode::OK);
        let body = call.json::<Value>().await.unwrap();
        let text = body["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            text,
            "subject=user-123;audiences=tmcp;scopes=resources:read,tools:call"
        );

        let expired = client
            .post(format!("{base_url}/"))
            .bearer_auth(token(&encoding_key, "kid-1", now() - 1))
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .json(&initialize_payload())
            .send()
            .await
            .unwrap();
        assert_eq!(expired.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert!(
            expired.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("error=\"invalid_token\"")
        );

        let sse = client
            .get(format!("{base_url}/"))
            .bearer_auth(&valid_token)
            .header("Accept", "text/event-stream")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .header("Mcp-Session-Id", &session_id)
            .send()
            .await
            .unwrap();
        assert_eq!(sse.status(), reqwest::StatusCode::OK);
        assert!(
            sse.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );

        let sse_missing_auth = client
            .get(format!("{base_url}/"))
            .header("Accept", "text/event-stream")
            .header("MCP-Protocol-Version", LATEST_PROTOCOL_VERSION)
            .header("Mcp-Session-Id", &session_id)
            .send()
            .await
            .unwrap();
        assert_eq!(sse_missing_auth.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert!(
            sse_missing_auth.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("resource_metadata=\"/.well-known/oauth-protected-resource\"")
        );

        let prm = client
            .get(format!("{base_url}/.well-known/oauth-protected-resource"))
            .send()
            .await
            .unwrap();
        assert_eq!(prm.status(), reqwest::StatusCode::OK);
        let prm_body = prm.json::<Value>().await.unwrap();
        assert_eq!(prm_body["resource"], "https://example.com/mcp");

        server_handle.stop().await.unwrap();
    }
}
