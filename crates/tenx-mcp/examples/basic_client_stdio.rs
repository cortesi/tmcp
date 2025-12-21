//! Basic MCP client example using stdio transport
//!
//! This client spawns the basic_server example in stdio mode and
//! demonstrates the same echo tool interaction as basic_client.rs
//! but using stdio communication instead of TCP.
//!
//! Usage:
//!   cargo run --example basic_client_stdio

use serde::{Deserialize, Serialize};
use tenx_mcp::{Arguments, Client, Result, ServerAPI, schema::Content, schemars};
use tokio::process::Command;
use tracing::info;
use tracing_subscriber::fmt;

/// Echo tool input parameters (must match server definition)
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct EchoParams {
    /// The message to echo back
    message: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    fmt::init();

    // Create the client
    let mut client = Client::new("basic-client-stdio", "0.1.0");

    // Configure the command to spawn the basic_server in stdio mode
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "-q", "--example", "basic_server", "--", "stdio"]);

    // Spawn the process and connect to it
    info!("Spawning basic_server in stdio mode...");
    let mut child = client.connect_process(cmd).await?;

    // Initialize the connection
    let init_result = client.init().await?;

    // Get server info from initialization result
    let server_info = &init_result.server_info;
    info!(
        "Connected to server: {} v{}",
        server_info.name, server_info.version
    );

    // List available tools
    info!("Listing available tools...");
    let tools = client.list_tools(None).await?;
    for tool in &tools.tools {
        info!(
            "Found tool: {} - {}",
            tool.name,
            tool.description.as_deref().unwrap_or("no description")
        );
    }

    // Call the echo tool
    let echo_message = "Hello from tenx-mcp stdio client!";
    info!("Calling echo tool with message: {}", echo_message);

    let params = EchoParams {
        message: echo_message.to_string(),
    };

    let args = Arguments::from_struct(params)?;
    let result = client.call_tool("echo", Some(args)).await?;

    if let Some(Content::Text(text_content)) = result.content.first() {
        info!("Echo response: {}", text_content.text);
    }

    // Clean shutdown
    info!("Shutting down...");

    // Kill the server process
    child.kill().await.expect("Failed to kill server process");

    Ok(())
}
