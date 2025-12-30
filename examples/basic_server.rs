//! Basic MCP server example that pairs with basic_client.rs
//!
//! This server can run in three modes:
//! 1. TCP mode: listens on TCP port 3000 (by default)
//! 2. HTTP mode: listens on HTTP port 8080 (by default)
//! 3. Stdio mode: communicates via stdin/stdout
//!
//! All modes provide a simple echo and ping tool that can be called by clients.
//!
//! Usage:
//!   cargo run --example basic_server tcp [host] [port]  # TCP mode
//!   cargo run --example basic_server http [host] [port] # HTTP mode
//!   cargo run --example basic_server stdio              # Stdio mode
//!   cargo run --example basic_server                    # Default to TCP mode on 127.0.0.1:3000

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use tmcp::{Result, Server, ServerCtx, ToolResponse, ToolResult, mcp_server, tool};
use tokio::signal::ctrl_c;
use tracing::info;
use tracing_subscriber::fmt;

/// Echo tool input parameters
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct EchoParams {
    /// The message to echo back
    message: String,
}

/// Echo tool response payload.
#[derive(Debug, Serialize, ToolResponse)]
struct EchoResponse {
    /// The echoed message.
    message: String,
}

/// Ping tool response payload.
#[derive(Debug, Serialize, ToolResponse)]
struct PingResponse {
    /// Ping result message.
    message: String,
}

/// Basic server connection that provides echo and ping tools
#[derive(Debug, Default)]
struct BasicServer {}

#[mcp_server]
/// Basic MCP server that provides echo and ping tools
impl BasicServer {
    #[tool]
    /// Echoes back the provided message
    async fn echo(&self, _context: &ServerCtx, params: EchoParams) -> ToolResult<EchoResponse> {
        Ok(EchoResponse {
            message: params.message,
        })
    }

    #[tool]
    /// Respond with a simple pong
    async fn ping(&self, _context: &ServerCtx) -> ToolResult<PingResponse> {
        Ok(PingResponse {
            message: "pong".to_string(),
        })
    }
}

#[derive(Parser)]
#[command(name = "basic_server")]
#[command(about = "Basic MCP server with echo tool", long_about = None)]
/// CLI options for the basic server example.
struct Cli {
    #[command(subcommand)]
    /// Optional subcommand selecting server mode.
    command: Option<Commands>,
}

#[derive(Subcommand)]
/// Supported runtime modes for the server.
enum Commands {
    /// Run server in TCP mode
    Tcp {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(short, long, default_value_t = 3000)]
        port: u16,
    },
    /// Run server in HTTP mode
    Http {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Run server in stdio mode
    Stdio,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Tcp {
        host: "127.0.0.1".to_string(),
        port: 3000,
    }) {
        Commands::Stdio => {
            // Run in stdio mode - no logging to avoid interfering with JSON-RPC
            Server::new(BasicServer::default).serve_stdio().await?;
        }
        Commands::Tcp { host, port } => {
            // Initialize logging for network modes
            fmt::init();

            let addr = format!("{host}:{port}");
            info!("Starting TCP MCP server on {}", addr);

            let handle = Server::new(BasicServer::default).serve_tcp(addr).await?;

            // Wait for Ctrl+C signal
            ctrl_c().await?;
            info!("Shutting down TCP server");

            // Gracefully stop the server
            handle.stop().await?;
        }
        Commands::Http { host, port } => {
            // Initialize logging for network modes
            fmt::init();

            let addr = format!("{host}:{port}");
            info!("Starting HTTP MCP server on {}", addr);

            let handle = Server::new(BasicServer::default).serve_http(addr).await?;

            // Wait for Ctrl+C signal
            ctrl_c().await?;
            info!("Shutting down HTTP server");

            // Gracefully stop the server
            handle.stop().await?;
        }
    }

    Ok(())
}
