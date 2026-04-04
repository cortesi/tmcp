//! Example protected MCP server using OAuth 2.1 bearer authentication.

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tmcp::{
    Arguments, Result, Server, ServerCtx, ServerHandler,
    auth::{
        ProtectedResourceMetadata,
        server::{AuthConfig, JwtValidator},
    },
    schema::{
        CallToolResult, ClientCapabilities, Cursor, Implementation, InitializeResult,
        ListToolsResult, ServerCapabilities, TaskMetadata, Tool, ToolSchema,
    },
};

/// Example server implementation.
struct OAuthServer;

#[async_trait]
impl ServerHandler for OAuthServer {
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
            Tool::new("hello", ToolSchema::default()).with_description("Return a greeting"),
        ))
    }

    async fn call_tool(
        &self,
        _context: &ServerCtx,
        name: String,
        _arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        if name == "hello" {
            Ok(CallToolResult::new().with_text_content("hello from an authenticated MCP server"))
        } else {
            Err(tmcp::Error::ToolNotFound(name))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let validator = Arc::new(JwtValidator::new(
        "https://issuer.example.com",
        ["tmcp"],
        "https://issuer.example.com/.well-known/jwks.json",
    ));

    let metadata = ProtectedResourceMetadata {
        resource: "https://example.com/mcp".to_string(),
        authorization_servers: vec!["https://issuer.example.com".to_string()],
        scopes_supported: Some(vec!["tools:call".to_string()]),
        bearer_methods_supported: Some(vec!["header".to_string()]),
        resource_documentation: None,
        additional: HashMap::new(),
    };

    let handle = Server::new(|| OAuthServer)
        .http("127.0.0.1:8080")
        .with_auth(AuthConfig::new(metadata, validator))
        .serve()
        .await?;

    handle.join().await
}
