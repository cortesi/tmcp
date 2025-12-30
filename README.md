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

- **Full MCP Protocol Support**: Implements the latest MCP specification (2025-06-18)
- **Client & Server**: Build both MCP clients and servers with ergonomic APIs
- **Multiple Transports**: TCP/IP and stdio transport layers
- **OAuth 2.0 Authentication**: Complete OAuth 2.0 support including:
  - Authorization Code Flow with PKCE
  - Dynamic client registration (RFC7591)
  - Automatic token refresh
  - MCP-specific `resource` parameter support
  - Built-in callback server for browser flows
- **Async/Await**: Built on Tokio for high-performance async operations

**Note**: Batch operations in the previous protocol version are not supported.

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
        protocol_version: String,
        _capabilities: ClientCapabilities,
        _client_info: Implementation,
    ) -> Result<InitializeResult> {
        let mut init = InitializeResult::new("weather_server")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_tools(true)
            .with_logging()
            .with_instructions("Minimal weather server example");
        if !protocol_version.is_empty() {
            init = init.with_mcp_version(protocol_version);
        }
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
