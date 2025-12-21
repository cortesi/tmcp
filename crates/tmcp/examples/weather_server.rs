//! Minimal weather server example.

use serde::{Deserialize, Serialize};
use tmcp::{Result, Server, ServerCtx, mcp_server, schema::*, schemars, tool};

#[derive(Default)]
/// Example server implementation for weather.
struct WeatherServer;

// Tool input schema is automatically derived from the struct using serde and schemars.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
/// Parameters for the weather tool.
struct WeatherParams {
    /// City name to query.
    city: String,
}

// The mcp_server macro generates the necessary boilerplate to expose methods as MCP tools.
#[mcp_server]
impl WeatherServer {
    // The doc comment becomes the tool's description in the MCP schema.
    #[tool]
    /// Get current weather for a city
    async fn get_weather(&self, _ctx: &ServerCtx, params: WeatherParams) -> Result<CallToolResult> {
        // Simulate weather API call
        let temperature = 22.5;
        let conditions = "Partly cloudy";

        Ok(CallToolResult::new().with_text_content(format!(
            "Weather in {}: {}°C, {}",
            params.city, temperature, conditions
        )))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = Server::default().with_connection(WeatherServer::default);
    server.serve_stdio().await?;
    Ok(())
}
