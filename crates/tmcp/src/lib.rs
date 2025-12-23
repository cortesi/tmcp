//! # tmcp
//!
//! A complete Rust implementation of the Model Context Protocol (MCP), providing both
//! client and server capabilities for building AI-integrated applications.
//!
//! ## Overview
//!
//! tmcp offers an ergonomic API for implementing MCP servers and clients with
//! support for tools, resources, and prompts. The library uses async/await patterns
//! with Tokio and provides procedural macros to eliminate boilerplate.
//!
//! ## Features
//!
//! - **Derive Macros**: Simple `#[mcp_server]` attribute for automatic implementation
//! - **Multiple Transports**: TCP, HTTP (with SSE), and stdio support
//! - **Type Safety**: Strongly typed protocol messages with serde
//! - **Async-First**: Built on Tokio for high-performance async I/O
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use serde::{Deserialize, Serialize};
//! use tmcp::{mcp_server, schema::*, schemars, tool, Result, Server, ServerCtx};
//!
//! #[derive(Default)]
//! struct WeatherServer;
//!
//! // Tool input schema is automatically derived from the struct using serde and schemars.
//! #[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
//! struct WeatherParams {
//!     city: String,
//! }
//!
//! // The mcp_server macro generates the necessary boilerplate to expose methods as MCP tools.
//! #[mcp_server]
//! impl WeatherServer {
//!     // The doc comment becomes the tool's description in the MCP schema.
//!     #[tool]
//!     /// Get current weather for a city
//!     async fn get_weather(&self, _ctx: &ServerCtx, params: WeatherParams) -> Result<CallToolResult> {
//!         // Simulate weather API call
//!         let temperature = 22.5;
//!         let conditions = "Partly cloudy";
//!
//!         Ok(CallToolResult::new().with_text_content(format!(
//!             "Weather in {}: {}°C, {}",
//!             params.city, temperature, conditions
//!         )))
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let server = Server::default().with_handler(WeatherServer::default);
//!     server.serve_stdio().await?;
//!     Ok(())
//! }
//! ```
//!
//! ## Transport Options
//!
//! - **TCP**: `server.listen_tcp("127.0.0.1:3000")`
//! - **HTTP**: `server.listen_http("127.0.0.1:3000")` (uses SSE for server->client)
//! - **Stdio**: `server.listen_stdio()` for subprocess integration

/// Server-side request/response API traits and helpers.
mod api;
/// Argument envelope used by tool calls and prompt arguments.
mod arguments;
/// Client implementation and transport orchestration.
mod client;
/// JSON-RPC codec for stream framing.
mod codec;
/// Connection traits for clients and servers.
mod connection;
/// Client/server context types.
mod context;
/// Error types and Result alias.
mod error;
/// HTTP transport implementation.
mod http;
/// JSON-RPC message definitions.
mod jsonrpc;
/// Request/response routing and tracking.
mod request_handler;
/// Server implementation and handle types.
mod server;
/// Transport traits and adapters.
mod transport;

pub mod auth;
/// Public schema types for MCP messages.
pub mod schema;
pub mod testutils;

pub use api::*;
pub use arguments::Arguments;
pub use client::Client;
pub use connection::{ClientHandler, ServerHandler};
pub use context::{ClientCtx, ServerCtx};
pub use error::{Error, Result};
pub use server::{Server, ServerHandle};
// Export user-facing macros directly from the crate root
pub use tmcp_macros::{mcp_server, tool};

// Keep the full macros module available for internal use
/// Re-exported macros module for internal use.
mod macros {
    pub use ::tmcp_macros::*;
}

// Re-export schemars for users
pub use schemars;

#[cfg(test)]
mod tests {
    use super::schema::*;

    #[test]
    fn test_jsonrpc_request_serialization() {
        let request = JSONRPCRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: RequestId::Number(1),
            request: Request {
                method: "initialize".to_string(),
                params: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: JSONRPCRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.jsonrpc, JSONRPC_VERSION);
        assert_eq!(parsed.id, RequestId::Number(1));
        assert_eq!(parsed.request.method, "initialize");
    }

    #[test]
    fn test_role_serialization() {
        let role = Role::User;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"user\"");

        let role = Role::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
    }
}
