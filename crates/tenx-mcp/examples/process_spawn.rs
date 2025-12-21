//! Example demonstrating how to spawn a process and connect to it as an MCP server
//!
//! This example shows how to:
//! - Spawn a child process running an MCP server
//! - Connect to it using the process's stdin/stdout
//! - Manage the process lifecycle

use tenx_mcp::{Arguments, Client, Result, ServerAPI};
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
    let mut child = match client.connect_process(cmd).await {
        Ok(child) => {
            info!("Successfully spawned and connected to process");
            child
        }
        Err(e) => {
            error!("Failed to spawn process: {}", e);
            return Err(e);
        }
    };

    // Initialize the connection
    match client.init().await {
        Ok(result) => {
            info!(
                "Connected to server: {} v{}",
                result.server_info.name, result.server_info.version
            );

            if let Some(instructions) = result.instructions {
                info!("Server instructions: {}", instructions);
            }
        }
        Err(e) => {
            error!("Failed to initialize: {}", e);
            // Kill the process if initialization fails
            if let Err(err) = child.kill().await {
                error!("Failed to kill child process after init error: {}", err);
            }
            return Err(e);
        }
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
    let args = Arguments::new().set("message", "Hello from spawned process!")?;

    match client.call_tool("echo", Some(args)).await {
        Ok(result) => {
            info!("Tool response: {:?}", result.content);
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

    // The process will be terminated when child is dropped
    // But we can also explicitly kill it if needed
    match child.kill().await {
        Ok(_) => info!("Process terminated"),
        Err(e) => error!("Failed to kill process: {}", e),
    }

    // Alternatively, you could wait for the process to exit naturally:
    // match child.wait().await {
    //     Ok(status) => info!("Process exited with status: {}", status),
    //     Err(e) => error!("Failed to wait for process: {}", e),
    // }

    Ok(())
}
