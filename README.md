![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/tmcp)](https://crates.io/crates/tmcp)
[![docs.rs](https://img.shields.io/docsrs/tmcp)](https://docs.rs/tmcp)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# tmcp

A Rust implementation of the Model Context Protocol for building AI-integrated applications.

--- 

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF). 

---

## Overview

A Rust implementation of the Model Context Protocol (MCP) - a JSON-RPC 2.0 based
protocol for AI models to interact with external tools and services. Supports
both client and server roles with async/await APIs.

---

## Features

- **Full MCP Protocol Support**: Implements the latest MCP specification (2025-11-25)
- **Client & Server**: Build both MCP clients and servers with ergonomic APIs
- **Multiple Transports**: TCP/IP and stdio transport layers
- **OAuth 2.0 Authentication**: Complete OAuth 2.0 support including:
  - Authorization Code Flow with PKCE
  - Dynamic client registration (RFC7591)
  - Automatic token refresh
  - MCP-specific `resource` parameter support
  - Built-in callback server for browser flows
  - Protected resource metadata discovery (RFC 9728)
  - Authorization server discovery (RFC 8414 / OpenID Connect)
  - Client ID metadata documents for HTTPS client IDs
- **Async/Await**: Built on Tokio for high-performance async operations

**Note**: Batch operations in the previous protocol version are not supported.

---

## Cargo features

tmcp's default feature set is empty. Core stdio and TCP clients and servers require no feature.
Enable only the optional capabilities an application uses:

- `http` provides streamable HTTP client and server transports.
- `auth` provides OAuth client flows and bearer-token protection, and enables `http`.
- `render` provides human-oriented MCP API rendering.
- `schema-validation` compiles advertised tool schemas for repeated output validation.
- `testutils` provides integration-test harnesses and transports.

```toml
tmcp = { version = "0.5", features = ["auth"] }
```

---

## OAuth discovery flow

When a protected resource challenges a request, inspect any `WWW-Authenticate` header for a
`resource_metadata` value. Fetch protected resource metadata from that URL, or fall back to
`/.well-known/oauth-protected-resource` (with optional path suffix) when no challenge is provided.
Use the advertised authorization server issuers to resolve RFC 8414 or OpenID Connect discovery
documents, then use those endpoints for authorization and registration. If the client identifier
is an HTTPS URL, fetch the client ID metadata document at that URL to obtain redirect URIs and
additional client settings.

---

## Delegated tool tables

Use delegated tools when tool implementations belong outside the server type. Add `#[tool]`
to each free function. List the function paths in `tools`, and set `tool_state_fn` to one server
method that resolves their shared state.

```rust
#[tool(defaults)]
/// Read one item from the selected workspace.
async fn read_item(state: &Workspace, params: ReadParams) -> ToolResult<ReadResult> {
    state.read(params).await
}

#[mcp_server(
    tools = [workspace_tools::read_item],
    tool_state_fn = resolve_workspace,
)]
impl AppServer {
    async fn resolve_workspace(
        &self,
        arguments: Option<&tmcp::Arguments>,
    ) -> ToolResult<Arc<Workspace>> {
        let workspace_id = arguments
            .and_then(|args| args.get::<String>("workspaceId"))
            .ok_or_else(|| ToolError::invalid_input("workspaceId is required"))?;
        self.workspace(&workspace_id).await
    }
}
```

The resolver runs only after the server matches a delegated tool name. It receives the unchanged
raw arguments before typed argument decoding. Its result must borrow the state type in the free
function. For example, `Arc<Workspace>` can supply `&Workspace`.

The macro derives each delegated tool schema from the free function. It also preserves tool
metadata, task support, argument handling, and output conversion. Duplicate local or delegated
tool names cause a compile error.

---

## Example 

From `./examples/weather_server.rs` 

<!-- snips: ./examples/weather_server.rs -->
```rust
//! Minimal weather server example.

use serde_json::json;
use tmcp::{
    Result, Server, ServerCtx, ToolError, ToolResult, mcp_server,
    schema::{ClientCapabilities, Implementation, InitializeResult, LoggingLevel, ServerNotification},
    tool, tool_params, tool_result,
};

/// Example server.
#[derive(Default)]
struct WeatherServer;

/// Parameters for the weather tool.
// Tool input schema is automatically derived from the struct using serde and schemars.
#[derive(Debug)]
#[tool_params]
struct WeatherParams {
    /// City name to query.
    city: String,
}

#[derive(Debug)]
#[tool_result]
/// Structured response for the weather tool.
struct WeatherResponse {
    /// City name queried.
    city: String,
    /// Temperature in Celsius.
    temperature_c: f64,
    /// Human-readable conditions.
    conditions: String,
}

/// Structured response for the ping tool.
#[derive(Debug)]
#[tool_result]
struct PingResponse {
    /// Ping response message.
    message: String,
}

/// Parameters for emitting a log message.
#[derive(Debug)]
#[tool_params]
struct LogParams {
    /// Message to include in the server log notification.
    message: String,
}

/// Result of emitting a log message.
#[derive(Debug)]
#[tool_result]
struct LogResponse {
    /// Whether the log notification was queued.
    logged: bool,
}

// The `mcp_server` macro generates the necessary boilerplate to expose methods as MCP tools.
#[mcp_server(initialize_fn = initialize)]
impl WeatherServer {
    /// Customize initialize to advertise logging support.
    async fn initialize(
        &self,
        _ctx: &ServerCtx,
        protocol_version: schema::ProtocolVersion,
        _capabilities: ClientCapabilities,
        _client_info: Implementation,
    ) -> Result<InitializeResult> {
        let init = InitializeResult::new("weather_server")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_tools(Some(true))
            .with_logging()
            .with_instructions("Minimal weather server example")
            .with_mcp_version(protocol_version);
        Ok(init)
    }

    // The doc comment becomes the tool's description in the MCP schema.
    #[tool]
    /// Get current weather for a city
    async fn get_weather(
        &self,
        params: WeatherParams,
    ) -> ToolResult<WeatherResponse> {
        // Simulate weather API call
        let temperature = 22.5;
        let conditions = "Partly cloudy";

        Ok(WeatherResponse {
            city: params.city,
            temperature_c: temperature,
            conditions: conditions.to_string(),
        })
    }

    #[tool]
    /// Respond with a simple pong
    async fn ping(&self) -> ToolResult<PingResponse> {
        Ok(PingResponse {
            message: "pong".to_string(),
        })
    }

    #[tool]
    /// Emit a logging notification using ServerCtx
    async fn log_message(&self, ctx: &ServerCtx, params: LogParams) -> ToolResult<LogResponse> {
        let payload = json!({ "message": params.message });
        ctx.notify(ServerNotification::logging_message(
            LoggingLevel::Info,
            Some("weather_server".to_string()),
            payload,
        ))
        .map_err(|e| ToolError::internal(e.to_string()))?;
        Ok(LogResponse { logged: true })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Server::new(WeatherServer::default).serve_stdio().await?;
    Ok(())
}

```

When you serve MCP over stdio, `stdout` is reserved for JSON-RPC messages. Do not attach a
tracing/logging subscriber that writes human-readable logs to `stdout`, and do not print with
`println!`. Route diagnostics to `stderr`, a file, or another sink instead.

### Protocol versions

`Client` and `Server` use the same ordered protocol-version configuration. The first client value
is its preferred version. The first server value is its latest version. Each value must be a valid
MCP release date.

```rust
use tmcp::schema::{ProtocolVersion, SupportedProtocolVersions};

let versions = SupportedProtocolVersions::new([
    "2025-11-25".parse::<ProtocolVersion>().unwrap(),
    "2025-06-18".parse::<ProtocolVersion>().unwrap(),
]).unwrap();

let client = tmcp::Client::new("example-client", "1.0.0")
    .with_protocol_versions(versions.clone());
let server = tmcp::Server::new(WeatherServer::default)
    .with_protocol_versions(versions);
```

The server returns the requested version when it supports that version. Otherwise, it returns its
latest version. The client disconnects if the returned version is not in its configured set.

With the `schema-validation` feature, `PreparedToolResultContract` compiles an output schema once.
It also selects the extraction mode for each call. `Text` and `Content` modes do not validate the
output schema.

Flat tool arguments can be declared directly in the tool signature for multi-argument tools:

```rust
#[tool]
async fn add(&self, a: f64, b: f64) -> ToolResult<AddResponse> {
    Ok(AddResponse { sum: a + b })
}
```

Single-argument tools remain struct-based by default; opt into flat handling explicitly:

```rust
#[tool(flat)]
async fn echo(&self, message: String) -> ToolResult<EchoResponse> {
    Ok(EchoResponse { message })
}
```
