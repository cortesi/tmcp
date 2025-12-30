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
