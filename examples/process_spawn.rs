//! Example demonstrating how to spawn a process and connect to it as an MCP server
//!
//! This example shows how to:
//! - Spawn a child process running an MCP server
//! - Connect to it using the process's stdin/stdout
//! - Manage the process lifecycle

use tmcp::{Arguments, Client, Result};
use tokio::process::Command;
use tracing::{Level, error, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    // Create the client
    let mut client = Client::new("process-spawn-example", "0.1.0");

    // Configure the command to spawn
    // In this example, we'll spawn another example server from this crate
    // In real usage, this would be your MCP server executable
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--example", "basic_server"]);

    info!("Spawning MCP server process...");

    // Spawn the process and connect to it
    let connection = match client.connect_process(cmd).await {
        Ok(connection) => {
            info!("Successfully spawned and connected to process");
            connection
        }
        Err(e) => {
            error!("Failed to spawn process: {}", e);
            return Err(e);
        }
    };

    let tmcp::SpawnedServer {
        mut process,
        server_info,
    } = connection;
    info!(
        "Connected to server: {} v{}",
        server_info.server_info.name, server_info.server_info.version
    );

    if let Some(instructions) = server_info.instructions {
        info!("Server instructions: {}", instructions);
    }

    // List available tools
    match client.list_tools(None).await {
        Ok(tools) => {
            info!("Available tools:");
            for tool in tools.tools {
                info!(
                    "  - {}: {}",
                    tool.name,
                    tool.description.as_deref().unwrap_or("(no description)")
                );
            }
        }
        Err(e) => {
            error!("Failed to list tools: {}", e);
        }
    }

    // Call a tool if available
    let args = Arguments::new().insert("message", "Hello from spawned process!");

    match client.call_tool("echo", args).await {
        Ok(result) => {
            // Use the ergonomic text() method to extract the response
            if let Some(text) = result.text() {
                info!("Tool response: {}", text);
            }
        }
        Err(e) => {
            error!("Failed to call tool: {}", e);
        }
    }

    // Ping the server to ensure it's still responsive
    match client.ping().await {
        Ok(_) => info!("Server is responsive"),
        Err(e) => error!("Ping failed: {}", e),
    }

    // Clean shutdown
    info!("Shutting down...");

    // The process will be terminated when dropped, but we can also explicitly kill it
    match process.kill().await {
        Ok(_) => info!("Process terminated"),
        Err(e) => error!("Failed to kill process: {}", e),
    }

    // Alternatively, you could wait for the process to exit naturally:
    // match process.wait().await {
    //     Ok(status) => info!("Process exited with status: {}", status),
    //     Err(e) => error!("Failed to wait for process: {}", e),
    // }

    Ok(())
}
