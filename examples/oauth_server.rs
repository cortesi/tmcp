//! Example MCP server with custom token validation.
//!
//! Demonstrates the pattern used by applications that protect an MCP endpoint with:
//! - A custom `TokenValidator` backed by application-specific logic (e.g. database sessions)
//! - `AuthConfig` with a public-facing `base_url` for correct RFC 9728 metadata
//! - Access to validated `AuthInfo` in tool handlers via `ServerCtx::extensions()`
//!
//! The server exposes a single `status` tool that returns the authenticated user's identity.
//!
//! Try it:
//!   cargo run --example oauth_server
//!   curl -X POST http://127.0.0.1:8080/mcp                    # → 401
//!   curl http://127.0.0.1:8080/.well-known/oauth-protected-resource/mcp  # → metadata

use std::sync::Arc;

use async_trait::async_trait;
use tmcp::{
    Arguments, Result, Server, ServerCtx, ServerHandler,
    auth::server::{AuthConfig, AuthError, AuthInfo, TokenValidator},
    schema::{
        CallToolResult, ClientCapabilities, Cursor, Implementation, InitializeResult,
        ListToolsResult, ServerCapabilities, TaskMetadata, Tool, ToolSchema,
    },
};

/// Application-specific token validator.
///
/// In production this would look up session UUIDs or OAuth access tokens in a database.
struct AppTokenValidator;

#[async_trait]
impl TokenValidator for AppTokenValidator {
    async fn validate(&self, token: &str) -> std::result::Result<AuthInfo, AuthError> {
        if token == "demo-admin-token" {
            Ok(AuthInfo {
                subject: "admin@example.com".to_string(),
                scopes: ["mcp"].into_iter().map(String::from).collect(),
                audiences: Vec::new(),
                extra: serde_json::json!({ "is_admin": true }),
            })
        } else {
            Err(AuthError::Invalid)
        }
    }
}

/// MCP server handler that reads auth info from the request context.
struct AppMcpServer;

#[async_trait]
impl ServerHandler for AppMcpServer {
    async fn initialize(
        &self,
        _context: &ServerCtx,
        _protocol_version: String,
        _capabilities: ClientCapabilities,
        _client_info: Implementation,
    ) -> Result<InitializeResult> {
        Ok(InitializeResult::new("app-mcp")
            .with_version("0.1.0")
            .with_capabilities(ServerCapabilities::default().with_tools(Some(false)))
            .with_instructions("Example MCP server with bearer-token auth."))
    }

    async fn list_tools(
        &self,
        _context: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        Ok(ListToolsResult::default().with_tool(
            Tool::new("status", ToolSchema::empty())
                .with_description("Return server status")
                .with_read_only_hint(true),
        ))
    }

    async fn call_tool(
        &self,
        context: &ServerCtx,
        name: String,
        _arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        if name != "status" {
            return Err(tmcp::Error::ToolNotFound(name));
        }

        // AuthInfo is injected by BearerAuthLayer → HTTP transport → ServerCtx extensions.
        let auth = context
            .extensions()
            .get::<AuthInfo>()
            .expect("auth info present after bearer middleware");

        Ok(CallToolResult::new()
            .with_text_content(format!("ok — authenticated as {}", auth.subject)))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let validator: Arc<dyn TokenValidator> = Arc::new(AppTokenValidator);

    // AuthConfig carries the public base URL so that WWW-Authenticate challenges and
    // protected resource metadata contain absolute URIs (required by RFC 9728).
    let auth_config = AuthConfig::new("https://example.com", validator).with_endpoint_path("/mcp");

    let handle = Server::new(|| AppMcpServer)
        .http("127.0.0.1:8080")
        .with_auth(&auth_config)
        .serve()
        .await?;

    tracing::info!("MCP server listening on http://127.0.0.1:8080/mcp");
    handle.join().await
}
