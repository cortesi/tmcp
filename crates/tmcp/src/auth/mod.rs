//! # OAuth Authentication Module for MCP
//!
//! This module provides comprehensive OAuth 2.0 authentication support for the Model Context Protocol (MCP),
//! implementing the authorization requirements specified in the MCP specification.
//!
//! ## Features
//!
//! ### OAuth 2.0 Authorization Code Flow with PKCE
//! - Full implementation of the authorization code flow with PKCE (Proof Key for Code Exchange)
//! - Automatic PKCE challenge generation and verification
//! - CSRF protection via state parameter
//! - Secure token storage and management
//!
//! ### MCP-Specific Requirements
//! - **Resource Parameter**: All authorization requests include the `resource` parameter for
//!   audience-bound tokens as required by MCP
//! - **Short-lived Tokens**: Support for token refresh to handle short-lived access tokens
//! - **Audience Binding**: Tokens are bound to specific MCP server endpoints
//!
//! ### Dynamic Client Registration (RFC7591)
//! - Automatic client registration with OAuth providers
//! - OAuth metadata discovery via `.well-known` endpoints
//! - Support for protected registration endpoints
//! - Fallback mechanisms for manual registration
//!
//! ### Discovery
//! - Protected resource metadata discovery (RFC 9728) with `WWW-Authenticate` handling
//! - Authorization server discovery via RFC 8414 and OpenID Connect Discovery
//! - Client ID metadata documents for HTTPS client identifiers
//!
//! ### Browser-based Authentication Flow
//! - Built-in OAuth callback server for handling browser redirects
//! - Automatic browser opening for user authorization
//! - Clean success page displayed after authorization
//! - Configurable callback port
//!
//! ### Token Management
//! - Automatic token refresh when tokens expire
//! - Thread-safe token storage using `Arc<RwLock<_>>`
//! - Seamless integration with HTTP transport for automatic token injection
//!
//! ## Usage Examples
//!
//! ### Basic OAuth Flow
//! ```no_run
//! use tmcp::auth::{OAuth2Config, OAuth2Client, OAuth2CallbackServer};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Configure OAuth client
//!     let config = OAuth2Config {
//!         client_id: "your_client_id".to_string(),
//!         client_secret: Some("your_client_secret".to_string()),
//!         auth_url: "https://auth.example.com/authorize".to_string(),
//!         token_url: "https://auth.example.com/token".to_string(),
//!         redirect_url: "http://localhost:8080/callback".to_string(),
//!         resource: "https://mcp.example.com".to_string(),
//!         scopes: vec!["read".to_string(), "write".to_string()],
//!     };
//!
//!     // Create OAuth client
//!     let mut oauth_client = OAuth2Client::new(config)?;
//!
//!     // Get authorization URL
//!     let (auth_url, _csrf_token) = oauth_client.get_authorization_url();
//!     println!("Open this URL in your browser: {}", auth_url);
//!
//!     // Start callback server
//!     let callback_server = OAuth2CallbackServer::new(8080);
//!     let (code, state) = callback_server.wait_for_callback().await?;
//!
//!     // Exchange code for token
//!     let token = oauth_client.exchange_code(code, state).await?;
//!     println!("Access token obtained!");
//!
//!     Ok(())
//! }
//! ```
//!
//! ### Dynamic Client Registration
//! ```no_run
//! use tmcp::auth::{OAuth2Client, ClientMetadata, DynamicRegistrationClient};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Option 1: Automatic registration with discovery
//!     let oauth_client = OAuth2Client::register_dynamic(
//!         "https://auth.example.com/authorize".to_string(),
//!         "https://auth.example.com/token".to_string(),
//!         "https://mcp.example.com".to_string(),
//!         "My MCP Client".to_string(),
//!         "http://localhost:8080/callback".to_string(),
//!         vec!["read".to_string()],
//!         None, // Will discover registration endpoint
//!     ).await?;
//!
//!     // Option 2: Manual registration with custom metadata
//!     let registration_client = DynamicRegistrationClient::default();
//!     let metadata = ClientMetadata::new("My Client", "http://localhost:8080/callback")
//!         .with_resource("https://mcp.example.com")
//!         .with_scopes(&["read".to_string()])
//!         .with_contacts(vec!["admin@example.com".to_string()]);
//!
//!     let response = registration_client
//!         .register("https://auth.example.com/register", metadata, None)
//!         .await?;
//!
//!     println!("Client ID: {}", response.client_id);
//!     Ok(())
//! }
//! ```
//!
//! ## Integration with MCP Client
//!
//! The OAuth module integrates seamlessly with the MCP client:
//!
//! ```no_run
//! use tmcp::{Client, auth::{OAuth2Config, OAuth2Client}};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = OAuth2Config {
//!         client_id: "your_client_id".to_string(),
//!         client_secret: Some("your_client_secret".to_string()),
//!         auth_url: "https://auth.example.com/authorize".to_string(),
//!         token_url: "https://auth.example.com/token".to_string(),
//!         redirect_url: "http://localhost:8080/callback".to_string(),
//!         resource: "https://mcp.example.com".to_string(),
//!         scopes: vec!["read".to_string()],
//!     };
//!     let oauth_client = Arc::new(OAuth2Client::new(config)?);
//!     
//!     let mut mcp_client = Client::new("my-app", "1.0.0");
//!     let init_result = mcp_client
//!         .connect_http_with_oauth("https://mcp.example.com", oauth_client)
//!         .await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Security Considerations
//!
//! - Always use HTTPS for production OAuth endpoints
//! - Store client secrets securely (environment variables, secure vaults)
//! - Implement proper CSRF token validation
//! - Use PKCE for all authorization code flows
//! - Validate SSL certificates in production
//!
//! ## Standards Compliance
//!
//! This implementation follows:
//! - OAuth 2.0 (RFC 6749)
//! - OAuth 2.0 PKCE (RFC 7636)
//! - OAuth 2.0 Dynamic Client Registration (RFC 7591)
//! - OAuth 2.0 Authorization Server Metadata (RFC 8414)
//! - OAuth 2.0 Protected Resource Metadata (RFC 9728)
//! - OpenID Connect Discovery
//! - Model Context Protocol Authorization Specification

/// OAuth 2.0 discovery helpers.
mod discovery;
/// OAuth 2.0 Dynamic Client Registration support.
mod dynamic_registration;
/// OAuth 2.0 client implementation and callback server.
mod oauth_client;

pub use discovery::{
    AuthorizationDiscoveryClient, AuthorizationServerDiscovery, AuthorizationServerMetadata,
    ClientIdMetadataDocument, ProtectedResourceDiscovery, ProtectedResourceMetadata,
};
pub use dynamic_registration::{
    ClientMetadata, ClientRegistrationResponse, DynamicRegistrationClient, RegistrationError,
};
pub use oauth_client::{OAuth2CallbackServer, OAuth2Client, OAuth2Config, OAuth2Token};
