//! # tmcp-core
//!
//! The protocol types of the Model Context Protocol (MCP): the [`schema`]
//! messages, [`Arguments`], [`Error`], and the [`McpApi`] snapshots, with
//! their rendering and the JSON Schema validation of tool results.
//!
//! This crate has no runtime and no features. It compiles for
//! `wasm32-unknown-unknown`. The `tmcp` crate builds the clients, servers,
//! and transports on these types and re-exports them, and the `tmcp-http`
//! crate adds the HTTP transport and OAuth.

#![warn(missing_docs)]

/// Argument envelope used by tool calls and prompt arguments.
mod arguments;
/// Error types and Result alias.
mod error;
/// Serializable snapshots of a server's MCP API.
mod mcp_api;
/// Human-oriented rendering for MCP API snapshots.
mod mcp_api_render;
/// Public schema types for MCP messages.
pub mod schema;

pub use arguments::Arguments;
pub use error::{
    Error, Result, TOOL_ERROR_INTERNAL, TOOL_ERROR_INVALID_INPUT, TOOL_ERROR_NOT_FOUND,
    TOOL_ERROR_TIMEOUT, ToolError, ToolResult,
};
pub use mcp_api::{McpApi, McpApiOptions, McpApiRefreshSnapshot, McpApiRefreshState};
pub use mcp_api_render::{McpApiRenderOptions, render_mcp_api};
pub use schema::ToolResponse;

/// Re-exported macros module for internal use.
mod macros {
    pub use ::tmcp_macros::*;
}

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
