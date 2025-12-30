//! Minimal weather server example.

use serde::{Deserialize, Serialize};
use tmcp::{Result, Server, ServerCtx, ToolResponse, ToolResult, mcp_server, tool};

/// Example server.
#[derive(Default)]
struct WeatherServer;

/// Parameters for the weather tool.
// Tool input schema is automatically derived from the struct using serde and schemars.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct WeatherParams {
    /// City name to query.
    city: String,
}

#[derive(Debug, Serialize, ToolResponse)]
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
#[derive(Debug, Serialize, ToolResponse)]
struct PingResponse {
    /// Ping response message.
    message: String,
}

// The `mcp_server` macro generates the necessary boilerplate to expose methods as MCP tools.
#[mcp_server]
impl WeatherServer {
    // The doc comment becomes the tool's description in the MCP schema.
    #[tool]
    /// Get current weather for a city
    async fn get_weather(
        &self,
        _ctx: &ServerCtx,
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
    async fn ping(&self, _ctx: &ServerCtx) -> ToolResult<PingResponse> {
        Ok(PingResponse {
            message: "pong".to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    Server::new(WeatherServer::default).serve_stdio().await?;
    Ok(())
}
