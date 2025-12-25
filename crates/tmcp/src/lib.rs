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
//! ## Transport Options
//!
//! - **TCP**: `server.listen_tcp("127.0.0.1:3000")`
//! - **HTTP**: `server.listen_http("127.0.0.1:3000")` (uses SSE for server->client)
//! - **Stdio**: `server.listen_stdio()` for subprocess integration

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

pub use arguments::Arguments;
pub use client::{Client, ProcessConnection};
pub use connection::{ClientHandler, ServerHandler};
pub use context::{ClientCtx, ServerCtx};
pub use error::{Error, Result};
pub use server::{Server, ServerHandle, TcpServerHandle};
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
