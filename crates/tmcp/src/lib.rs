//! # tmcp
//!
//! A complete Rust implementation of the Model Context Protocol (MCP),
//! providing both client and server capabilities for building AI-integrated
//! applications.
//!
//! ## Overview
//!
//! tmcp offers an ergonomic API for implementing MCP servers and clients with
//! support for tools, resources, and prompts. The library uses async/await
//! patterns with Tokio and provides procedural macros to eliminate boilerplate.
//!
//! ## Features
//!
//! - **Derive Macros**: Simple `#[mcp_server]` attribute for automatic
//!   implementation
//! - **Multiple Transports**: TCP, HTTP (with SSE), and stdio support
//! - **Type Safety**: Strongly typed protocol messages with serde
//! - **Async-First**: Built on Tokio for high-performance async I/O
//!
//! ## Transport Options
//!
//! - **TCP**: `server.listen_tcp("127.0.0.1:3000")`
//! - **Stdio**: `server.listen_stdio()` for subprocess integration
//! - **HTTP**: the `tmcp-http` crate, which also adds OAuth
//!
//! ## Crates
//!
//! The crates have no features. `tmcp-core` holds the protocol types:
//! [`schema`], [`Arguments`], [`Error`], and the [`McpApi`] snapshots. It has
//! no runtime and compiles for `wasm32-unknown-unknown`. This crate builds the
//! Tokio clients, servers, connections, and transports on it, and re-exports
//! it. `tmcp-http` adds the streamable HTTP transport and OAuth.
//!
//! ## Building Servers: Macro vs Trait
//!
//! tmcp provides two approaches for implementing MCP servers:
//!
//! ### The `#[mcp_server]` Macro
//!
//! Best for servers that primarily expose derived tools and optionally forward
//! resource handling to dynamic methods. The macro automatically:
//! - Generates [`ServerHandler`] trait implementation
//! - Derives tool schemas from function signatures using `schemars`
//! - Registers tools in `list_tools` and routes calls in `call_tool`
//! - Forwards resource protocol methods configured with `resources_fn`,
//!   `read_resource_fn`, or `resource_templates_fn`
//! - Provides sensible defaults for `initialize`
//!
//! ```ignore
//! use schemars::JsonSchema;
//! use serde::Deserialize;
//! use tmcp::{mcp_server, schema::CallToolResult, tool, ServerCtx, ToolResult};
//!
//! #[derive(Debug, Deserialize, JsonSchema)]
//! struct GreetParams {
//!     name: String,
//! }
//!
//! #[mcp_server]
//! impl MyServer {
//!     #[tool]
//!     async fn greet(&self, _ctx: &ServerCtx, params: GreetParams) -> ToolResult {
//!         Ok(CallToolResult::new().with_text_content(format!(
//!             "Hello, {}!",
//!             params.name
//!         )))
//!     }
//! }
//! ```
//!
//! ### The [`ServerHandler`] Trait
//!
//! Use the trait directly when you need:
//! - **Custom initialization**: Validate clients, negotiate capabilities, or
//!   reject connections
//! - **Per-connection state**: Access to `ServerCtx` in all methods for
//!   client-specific data
//! - **Prompts or uncommon protocol hooks**: Full access to MCP features beyond
//!   tools and resources
//! - **Fine-grained error handling**: Custom error responses and logging
//!
//! ```ignore
//! use tmcp::{ServerHandler, ServerCtx, Result};
//! use async_trait::async_trait;
//!
//! struct MyServer;
//!
//! #[async_trait]
//! impl ServerHandler for MyServer {
//!     async fn initialize(&self, ctx: &ServerCtx, ...) -> Result<InitializeResult> {
//!         // Custom capability negotiation
//!     }
//!
//!     async fn list_tools(&self, ctx: &ServerCtx, ...) -> Result<ListToolsResult> {
//!         // Dynamic tool registration
//!     }
//! }
//! ```
//!
//! See [`ServerHandler`] documentation for the default behavior philosophy.

#![warn(missing_docs)]

/// Client implementation and transport orchestration.
mod client;
/// JSON-RPC codec for stream framing.
mod codec;
/// Connection traits for clients and servers.
mod connection;
/// Client/server context types.
mod context;
/// Custom requests from a server to its client.
mod custom;
/// JSON-RPC message definitions.
mod jsonrpc;
/// Inspection of MCP APIs from server handlers and connected clients.
mod mcp_api_inspect;
/// Request/response routing and tracking.
mod request_handler;
/// Server implementation and handle types.
mod server;
/// Test utilities for building tmcp integration tests.
pub mod testutils;
/// Tool registration and progressive discovery support.
mod toolset;
/// Transport traits and adapters.
pub mod transport;

pub use client::{Client, ClientRequestHandle, SpawnedServer};
pub use connection::{ClientHandler, ServerHandler};
pub use context::{ClientCtx, ServerCtx};
pub use custom::{CUSTOM_PAYLOAD_LIMIT, CustomRequest};
pub use mcp_api_inspect::{
    collect_client_prompts, collect_client_resource_templates, collect_client_resources,
    collect_client_tools, inspect_client, inspect_server, inspect_server_with,
};
#[cfg(unix)]
pub use server::UnixServerHandle;
#[doc(hidden)]
pub use server::{HandlerFactory, NotificationFanout};
pub use server::{Server, ServerHandle, TcpServerHandle, handle_request};
pub use tmcp_core::{
    Arguments, Error, McpApi, McpApiOptions, McpApiRefreshSnapshot, McpApiRefreshState,
    McpApiRenderOptions, Result, TOOL_ERROR_INTERNAL, TOOL_ERROR_INVALID_INPUT,
    TOOL_ERROR_NOT_FOUND, TOOL_ERROR_TIMEOUT, ToolError, ToolResponse, ToolResult, render_mcp_api,
    schema,
};
pub use tmcp_macros::{
    Group, ToolResponse, delegate_server_handler, group, mcp_server, tool, tool_group, tool_params,
    tool_result,
};
pub use toolset::{
    ActivationHook, Group, GroupConfig, GroupInfo, ToolCallFuture, ToolFuture, ToolSet,
    ToolSetView, Visibility,
};

/// Static schema and dispatch contract for a generated delegated-tool group.
pub trait ToolGroup {
    /// Shared state resolved by the enclosing server for each tool call.
    type State: Send + Sync + 'static;

    /// Exact tool names owned by this group.
    const NAMES: &'static [&'static str];

    /// Builds the MCP schemas for this group's tools.
    fn schemas() -> Vec<schema::Tool>;

    /// Returns true when any group tool supports task-augmented calls.
    fn supports_tasks() -> bool;

    /// Dispatches one group-owned tool call.
    fn call<'a>(
        state: Self::State,
        context: &'a ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
        task: Option<schema::TaskMetadata>,
    ) -> ToolCallFuture<'a>;
}
#[doc(hidden)]
pub use toolset::{GroupDispatch, GroupRegistration};

/// Crates re-exported for use by tmcp's generated macro code.
///
/// This module is an implementation detail of the tmcp macros and carries no
/// stability guarantees; do not use it directly.
#[doc(hidden)]
pub mod __private {
    pub use async_trait;
    pub use schemars;
    pub use serde;
    pub use serde_json;
}
